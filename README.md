![Logo](./banner.png)

# OpenHP1 Mobile

Play the first Harry Potter PC game on your phone or tablet, straight from the browser. OpenHP1 Mobile is the browser and iOS build of OpenHP1, an open-source Rust reimplementation of the original game engine. It runs from your own legally obtained copy of the game and ships none of its files.

OpenHP1 requires the original game files to run.

## Requirements

| Device                | Browser                             | Notes                                                    |
|-----------------------|-------------------------------------|----------------------------------------------------------|
| iPhone / iPad         | Safari on iOS or iPadOS 26 or later | Add the page to the Home Screen for a true full screen.  |
| Android phone/tablet  | A current Chromium-based browser    | Needs WebGPU.                                            |
| Desktop (for testing) | A current browser with WebGPU       | Uses keyboard and mouse instead of touch controls.       |

The page must be served over HTTPS (or from `localhost`), because browsers only expose WebGPU to secure contexts. The browser build has not yet been verified on a physical iOS device. See [`docs/web.md`](docs/web.md).

## Playing

1. Open the hosted OpenHP1 page.
2. Import your game folder. On iPhone and iPad, put a ZIP of the installed game folder (the one containing `System`, `Maps`, and `Textures`) in the Files app and import that ZIP.
3. Select **Play**.

Your files stay in the browser's storage and are never uploaded. Settings and saves live in the same storage. Safari's Private Browsing cannot store the files, so use a normal tab.

### Touch controls

| Control                                            | Action                                             |
|----------------------------------------------------|----------------------------------------------------|
| Floating stick (left half of the screen)           | Walk in any direction; push further to move faster |
| Drag anywhere else                                 | Look around                                        |
| Jump / Cast (bottom right)                         | Jump, and cast spells (hold and drag to trace)     |
| Boost / Brake (bottom right, only while on broom)  | Fly faster or slower                               |
| Pause (top right)                                  | Open the pause menu                                |
| Skip (top right, during cutscenes)                 | Skip the cutscene                                  |

The stick works like the virtual sticks in console emulators: its base follows your thumb when you pull past the rim, movement eases in smoothly, and Harry stops the moment you let go. Controls stay clear of the notch, rounded corners, and home indicator.

## Mobile features

- **True full screen.** The image takes your screen's own aspect ratio, so there are no black bars on tall phone screens.
- **Adaptive resolution.** If frames fall below about 45 fps, the game lowers its internal resolution (by up to half) and raises it again when things settle, so play stays smooth. Your saved resolution is never changed.
- **Two renderers.** The Classic renderer is true to the original and the lightest on a phone's GPU. The Modern renderer adds HDR, tone mapping, ambient occlusion, bloom, and more. Classic also offers an optional emulated 16-bit color mode.
- **Original audio.** Menu and storybook music and in-game sound play through Web Audio, including with the iPhone ringer switch silenced.
- **Cutscene and storybook skipping**, with on-screen buttons.
- **Saves and settings in the browser**, flushed before the page closes.

The game is playable from start to finish on desktop builds. Minor graphical and behavioral differences from the original may remain on every platform.

## Known limits

- Map loading blocks the page while it runs.
- Mobile Safari's memory budget is tight; a large level uses roughly 500 MB.
- iOS Low Power Mode caps the frame rate at 30 fps.
- Safari may evict stored site data that has not been used for a while; adding the page to the Home Screen gives it persistent storage.
- Screenshots and the developer console's `report` command do not work in the browser.

## Building the web version

Install a modern Rust toolchain with [rustup](https://rustup.rs/), the `wasm32-unknown-unknown` target, and `wasm-bindgen-cli` at the version recorded in `Cargo.lock`:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version <version in Cargo.lock> --locked
./scripts/build-web.sh --release   # writes target/web
```

`--release` uses the optimized `web` profile (fat LTO) and runs `wasm-opt -O3` if it is installed. Serve `target/web` as static files over HTTPS. The output contains no original game files; hosting it publishes only OpenHP1 code.

## Docs

See [`docs/web.md`](docs/web.md) for the browser build, storage, and touch-control details, and the rest of [`docs/`](docs/) for the reverse-engineered package, map, texture, and rendering notes. [`SETTINGS.md`](SETTINGS.md) documents `OpenHP1.ini`.

## Legal

OpenHP1 is an independent open-source project and is not affiliated with, endorsed by, or sponsored by Warner Bros., Electronic Arts, or the owners of Harry Potter. All trademarks belong to their respective owners. Original game data must never be committed or distributed with OpenHP1.
