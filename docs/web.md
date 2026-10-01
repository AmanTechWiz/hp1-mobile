# Browser and iOS build

`openhp1-game` also builds for `wasm32-unknown-unknown` and runs in browsers
that provide WebGPU, including Safari on iOS and iPadOS 26 or later. The web
build runs the same scene, runtime, renderer, and UI code as the native game.
Only the platform seams differ.

## Building and serving

```sh
cargo install wasm-bindgen-cli --version <version in Cargo.lock> --locked
rustup target add wasm32-unknown-unknown
scripts/build-web.sh --release            # writes target/web
```

`scripts/build-web.sh` builds the game for wasm, runs `wasm-bindgen --target web`,
and copies the static shell from [`web/`](../web/). It runs `wasm-opt` for
release builds when that tool is installed. Serve the output directory as static
files. Browsers expose WebGPU only to secure contexts, so serve it over HTTPS,
or from `localhost` when testing on the same machine. An iPhone on the local
network cannot use a plain `http://` LAN address.

The output never contains original game files. Hosting it publishes only
OpenHP1 code.

## Game files

The page asks the player to import their own installation, either as a folder
(desktop browsers) or as a ZIP of that folder (iOS: compress it in the Files
app or on a computer). The importer looks for `System/Default.ini` to find the
installation root, ignores `__MACOSX` and `._*` entries, inflates entries with
`DecompressionStream`, and stores every file as a `Blob` in the `openhp1`
IndexedDB database under `/game/...`.

Settings, saves, and other files the game writes live under `/settings/...` in
the same database. Removing the game files from the page keeps them.

IndexedDB confirms every write asynchronously, after the game has already read
it back from memory, so a save reads back for the rest of the session whether
or not it ever reaches the database. When the browser refuses a write (Private
Browsing, a full disk, or an evicted database), the page shows a warning naming
the affected files: a save named there exists only in memory and is gone after
a reload. The wasm build cannot see this failure itself — `web.rs` declares
`host_persist` with no return value and no `catch`, so the host reports refused
writes on the page instead of into Rust.

Safari's Private Browsing refuses to store Blobs in IndexedDB, so imports fail
there with an explanatory message. Safari may also evict website data that has
not been used for a while. Adding the page to the Home Screen gives it
persistent storage and a full-screen window.

## Filesystem seam

Browsers have no filesystem, and `std::fs` only returns errors on
`wasm32-unknown-unknown`. Code that touches game or settings files uses
`openhp1_package::fs`:

- Native builds re-export `std::fs` unchanged.
- Web builds mount a `MemoryFs` populated from the shell's file list. Stored
  files are read through the host on every access and are not cached in wasm
  memory. Only parsed packages stay resident, because a whole installation does
  not fit in mobile Safari's memory budget. Writes stay in memory for the
  session and are dispatched to IndexedDB when they arrive: the browser runs
  overlapping readwrite transactions on one store in creation order, so each
  write starts its own transaction immediately instead of queueing behind
  earlier ones. The host tracks every dispatched write and flushes them before
  reloading — `exited` and the Reload button await the queue, capped at two
  seconds so a stuck transaction cannot hang the exit — while `beforeunload`
  holds the page open while a write is still in flight and `pagehide`
  re-dispatches any refused write as a last attempt. A write that the browser
  still refuses is surfaced on the page as described under Game files;
  `openhp1Host.storageStatus()` reports `{ pending, failed }` for anything that
  wants to query it, but no Rust code calls it today.
- `fs::read_prefix` reads only the start of a file. Package discovery checks the
  four-byte magic this way instead of reading every package whole.
- `fs::is_absolute` treats rooted paths as absolute, because `Path::is_absolute`
  reports every path as relative on `wasm32-unknown-unknown`.
- The game root is `/game` (`fs::WEB_GAME_ROOT`) and the settings directory is
  `/settings` (`fs::WEB_SETTINGS_DIR`).

The host reads Blobs synchronously with a synchronous `XMLHttpRequest` on a Blob
URL, decoding the response as `x-user-defined` (each byte becomes one UTF-16
code unit whose low byte is the original byte). A window-scoped synchronous XHR
cannot return an `ArrayBuffer`, and the main thread cannot wait for promises.
Text decoding honors byte order marks, so files that start with one, along with
files of 256 KiB or less, are read into memory when the game starts. For other
files the host caches a 16-byte prefix. If a startup probe shows that
synchronous Blob reads do not work, every file is kept in memory instead.

## Clocks, threads, and blocking

`std::time::Instant` and `SystemTime` panic on `wasm32-unknown-unknown`. Runtime
code uses `web_time`, which re-exports `std::time` on native targets. The web
build has no threads and cannot block:

- The WebGPU adapter and device are requested asynchronously before the event
  loop starts, then shared by every level's renderer. Native builds still create
  a device per level.
- winit runs through `EventLoopExtWebSys::spawn_app` on the page's
  `openhp1-canvas`, which CSS sizes to the viewport.
- Frames are paced by `requestAnimationFrame`: each frame requests the next
  redraw immediately. The native 60 Hz deadline timer would expire just after
  a display refresh and skip it.
- Screenshots need a blocking GPU readback, so they report an error in the
  browser.
- Map loading still runs synchronously and blocks the page while it loads.
- The developer console's `report` command writes through `std::fs` and fails
  in the browser.

kira's cpal backend uses Web Audio on wasm. Browsers start audio suspended, so
the shell resumes every `AudioContext` on each pointer, touch, and key gesture,
and on iOS sets `navigator.audioSession.type = "playback"` so audio plays even
with the ringer switch silenced.

## WebGPU shader constraints

WebGPU only allows implicit-derivative operations (`textureSample`,
`textureSampleBias`, `dpdx`, `dpdy`, `fwidth`) in uniform control flow, and
browser shader compilers enforce this more strictly than native naga. Keep those
operations at the top of entry points, before any per-pixel branch, loop, or
early return. Inside non-uniform flow, use explicit-level or explicit-gradient
sampling only when it provably returns the same texels. The lightmap and
visibility atlases qualify: they have one mip level and matching minification
and magnification filters.

Every Classic and Modern pipeline, the game's presentation shader, and egui's
shader compile and render without validation errors in WebKit and Chromium.

## Touch controls

When the browser exposes touch input (`navigator.maxTouchPoints` or
`ontouchstart`), the game skips cursor capture and draws on-screen controls over
the presented game image:

| Control | Desktop equivalent |
| --- | --- |
| Floating analog stick, anchored where the thumb lands on the left half | `W`/`A`/`S`/`D`, scaled by how far the stick is pushed (12% radial dead zone) |
| Drag elsewhere | Mouse motion (2 counts per logical pixel) |
| Jump | `Space` |
| Cast | Left mouse button |
| Boost / Brake (only while the player is `BroomHarry`) | `Z` / `X` |
| Pause (top right, clear of the health HUD) | `Escape` (pause menu) |

The stick's `[right, forward]` vector scales the same `aBaseY`, `aStrafe`, and
`aBaseX` axes the keys drive, so a partial push walks slower. Broom pitch is
digital and holds once the stick passes halfway. Touches drive the same
`InputState` as the desktop bindings, so gameplay sees the original input axes. Menus receive touches through egui as pointer input.
Holding Cast while dragging to look traces spell gestures. Controls are laid out
inside the letterboxed game area, because they are drawn into the game image.
Touch devices therefore default to a 1280x720 internal resolution when no
resolution is saved, which fills wide phone screens.

Browsers without the Pointer Lock API (iPhone Safari) never request cursor
capture: winit calls the API unconditionally, and the resulting exception left
winit's internal borrows held and panicked the event loop.

## Testing

- `openhp1_package::fs::MemoryFs` and the touch mapping have native unit tests.
- The import, storage, mount, synchronous-read, and error paths were exercised
  in Playwright's WebKit and Chromium with a synthetic ZIP installation.
- Shader compilation was checked by rendering synthetic scenes in both
  engines.

- With a local installation, the menu, storybook, and `Lev_Tut1` were played in
  WebKit (iPhone 15 Pro landscape emulation), Chromium with real touch events,
  and Brave. `Lev_Tut1` holds 60 fps and uses about 520 MB of wasm memory.
  Under 2x CPU throttling it stays at 52-60 fps; under 4x it drops to about 29.

Chromium's device emulation with a device scale factor above 1 sizes the canvas
backing store at CSS pixels while reporting the scaled `devicePixelRatio`, so
input lands at the wrong position. Test touch input there with a scale factor
of 1. Real browsers are unaffected.

The game has not yet been verified on a physical iOS device.
