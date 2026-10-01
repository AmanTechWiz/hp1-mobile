//! Browser startup for the wasm32 build.
//!
//! The page's `openhp1Host` script owns the player's imported installation and
//! persisted settings. This module mounts them through `openhp1_package::fs`,
//! requests the WebGPU device that browsers only provide asynchronously, loads
//! the startup map, and hands the page's canvas to the same winit event loop
//! that native builds run.

use std::{cell::RefCell, io, path::Path};

use anyhow::{Context, Result};
use openhp1_package::{
    fs::{self, MemoryFs, Storage},
    resolve_game_installation,
};
use openhp1_scene::LoadedScene;
use tracing::{Level, Metadata, error, info};
use tracing_subscriber::{
    EnvFilter, Layer, fmt::MakeWriter, layer::SubscriberExt, util::SubscriberInitExt,
};
use wasm_bindgen::{JsCast, JsValue, prelude::wasm_bindgen};
use wasm_bindgen_futures::JsFuture;
use winit::{event_loop::EventLoop, platform::web::EventLoopExtWebSys};

use crate::app::GameApp;

const CANVAS_ID: &str = "openhp1-canvas";

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = openhp1Host, js_name = files)]
    fn host_files() -> js_sys::Array;

    #[wasm_bindgen(js_namespace = openhp1Host, js_name = read, catch)]
    fn host_read(path: &str, limit: Option<u32>) -> Result<js_sys::Uint8Array, JsValue>;

    #[wasm_bindgen(js_namespace = openhp1Host, js_name = persist)]
    fn host_persist(path: &str, contents: Option<js_sys::Uint8Array>);

    #[wasm_bindgen(js_namespace = openhp1Host, js_name = status)]
    fn host_status(message: &str);

    #[wasm_bindgen(js_namespace = openhp1Host, js_name = nextFrame)]
    fn host_next_frame() -> js_sys::Promise;

    #[wasm_bindgen(js_namespace = openhp1Host, js_name = ready)]
    fn host_ready();

    #[wasm_bindgen(js_namespace = openhp1Host, js_name = fail)]
    fn host_fail(message: &str);

    #[wasm_bindgen(js_namespace = openhp1Host, js_name = exited)]
    fn host_exited();
}

pub(crate) fn start() {
    std::panic::set_hook(Box::new(|panic| {
        let message = panic.to_string();
        web_sys::console::error_1(&JsValue::from_str(&message));
        host_fail(&message);
    }));
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .without_time()
                .with_ansi(false)
                .with_writer(ConsoleLog)
                .with_filter(EnvFilter::new("info,symphonia_bundle_mp3=off")),
        )
        .try_init();
    wasm_bindgen_futures::spawn_local(async {
        if let Err(error) = run().await {
            error!(%error, "could not start OpenHP1");
            host_fail(&format!("{error:#}"));
        }
    });
}

async fn run() -> Result<()> {
    mount_host_files()?;
    host_status("Starting WebGPU");
    let gpu = Gpu::request().await?;
    GPU.with_borrow_mut(|slot| *slot = Some(gpu));

    host_status("Loading the startup map");
    // Map loading blocks the page, so let it paint the status first.
    let _ = JsFuture::from(host_next_frame()).await;
    let level = resolve_game_installation()?.startup_map().to_path_buf();
    let scene = LoadedScene::load(level)?;
    crate::log_scene_diagnostics(&scene);

    let event_loop = EventLoop::new()?;
    host_ready();
    event_loop.spawn_app(GameApp::new(scene, None));
    Ok(())
}

fn mount_host_files() -> Result<()> {
    let mut filesystem = MemoryFs::new(Box::new(HostStorage));
    let mut count = 0;
    for entry in host_files().iter() {
        let entry = entry.unchecked_into::<js_sys::Array>();
        let path = entry
            .get(0)
            .as_string()
            .context("the host listed a file without a path")?;
        let len = entry
            .get(1)
            .as_f64()
            .with_context(|| format!("the host listed `{path}` without a size"))?;
        filesystem
            .add_stored(&path, len as u64)
            .with_context(|| format!("could not mount `{path}`"))?;
        count += 1;
    }
    fs::mount(filesystem);
    info!(files = count, "mounted browser storage");
    Ok(())
}

struct HostStorage;

impl Storage for HostStorage {
    fn read(&self, path: &Path, limit: Option<usize>) -> io::Result<Vec<u8>> {
        let limit = limit.map(|limit| u32::try_from(limit).unwrap_or(u32::MAX));
        host_read(&path.to_string_lossy(), limit)
            .map(|bytes| bytes.to_vec())
            .map_err(|error| {
                io::Error::other(format!(
                    "could not read `{}` from browser storage: {}",
                    path.display(),
                    js_message(&error)
                ))
            })
    }

    fn persist(&self, path: &Path, contents: Option<&[u8]>) {
        host_persist(
            &path.to_string_lossy(),
            contents.map(js_sys::Uint8Array::from),
        );
    }
}

fn js_message(error: &JsValue) -> String {
    error
        .dyn_ref::<js_sys::Error>()
        .map(|error| String::from(error.message()))
        .or_else(|| error.as_string())
        .unwrap_or_else(|| format!("{error:?}"))
}

thread_local! {
    static GPU: RefCell<Option<Gpu>> = const { RefCell::new(None) };
}

/// The WebGPU device shared by every level's renderer.
///
/// Browsers only create adapters and devices asynchronously, so the device is
/// requested before the event loop starts instead of per level.
#[derive(Clone)]
pub(crate) struct Gpu {
    pub(crate) instance: wgpu::Instance,
    pub(crate) adapter: wgpu::Adapter,
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
}

impl Gpu {
    async fn request() -> Result<Self> {
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .context("this browser does not provide WebGPU")?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("OpenHP1 game device"),
                ..Default::default()
            })
            .await
            .context("failed to create the graphics device")?;
        Ok(Self {
            instance,
            adapter,
            device,
            queue,
        })
    }
}

pub(crate) fn exited() {
    host_exited();
}

pub(crate) fn gpu() -> Option<Gpu> {
    GPU.with_borrow(Clone::clone)
}

pub(crate) fn canvas() -> Option<web_sys::HtmlCanvasElement> {
    web_sys::window()?
        .document()?
        .get_element_by_id(CANVAS_ID)?
        .dyn_into()
        .ok()
}

/// Whether the device has a touch screen and needs on-screen controls.
pub(crate) fn touch_screen() -> bool {
    web_sys::window().is_some_and(|window| window.navigator().max_touch_points() > 0)
}

struct ConsoleLog;

impl<'a> MakeWriter<'a> for ConsoleLog {
    type Writer = ConsoleLine;

    fn make_writer(&'a self) -> Self::Writer {
        ConsoleLine {
            level: Level::INFO,
            line: Vec::new(),
        }
    }

    fn make_writer_for(&'a self, meta: &Metadata<'_>) -> Self::Writer {
        ConsoleLine {
            level: *meta.level(),
            line: Vec::new(),
        }
    }
}

/// Buffers one formatted event and sends it to the browser console.
struct ConsoleLine {
    level: Level,
    line: Vec<u8>,
}

impl io::Write for ConsoleLine {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.line.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for ConsoleLine {
    fn drop(&mut self) {
        let line = String::from_utf8_lossy(&self.line);
        let line = JsValue::from_str(line.trim_end());
        match self.level {
            Level::ERROR => web_sys::console::error_1(&line),
            Level::WARN => web_sys::console::warn_1(&line),
            _ => web_sys::console::log_1(&line),
        }
    }
}
