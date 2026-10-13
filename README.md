# Cutdeck

A desktop video editor in the spirit of InShot, built with **Rust + Tauri**. The editing UI runs in the system webview; everything heavy (decoding, effects, encoding, background removal) runs natively.

## What it does

| | |
|---|---|
| **Timeline** | Import videos and photos, drag to reorder, trim with handles, split (S), duplicate, delete, undo/redo |
| **Speed** | 0.25x to **100x**, slider + quick picks + exact value. Export is frame-accurate at every speed |
| **Canvas** | 9:16, 1:1, 4:5, 16:9, 4:3, 3:4 or original. Blurred or colour background behind each clip |
| **Look** | 12 filters, brightness / contrast / saturation / warmth / vignette, rotate, mirror, fit/fill, zoom, drag to reposition |
| **Background removal** | **AI cutout** for video and photos (people, pets, products, cars), replaced by blur, colour, image or the canvas. **Green screen** keying. Transparent PNG export for photos |
| **Text** | 6 fonts, 4 styles, drag on the preview, show/hide times |
| **Audio** | Per-clip volume, music track with offset and fades |
| **Export** | MP4 (H.264 + AAC) at 720p / 1080p / 2K / 4K, 24/30/60 fps, on your **GPU encoder** when available |
| **Projects** | Save/open `.cutdeck` files (Ctrl+S / Ctrl+O). Media stays where it is on disk |

## Why Rust makes it faster

* **Exports are one native FFmpeg pass.** `src-tauri/src/graph.rs` turns the whole project into a single FFmpeg filtergraph. Each clip is seeked directly, retimed and **resampled to the output frame rate first**, so at 100x only the frames that end up in the video are scaled and filtered.
* **Turbo for 50x+.** At very high speeds Cutdeck decodes only keyframes. A 5-minute 1080p clip at 100x exported in 2.5 s instead of 17.9 s in testing.
* **GPU encoding.** On first launch Cutdeck test-encodes with NVIDIA NVENC, Apple VideoToolbox, Intel Quick Sync and AMD AMF and uses the first that works (falls back to x264 automatically). The header shows which one you have.
* **Native AI cutout.** U²-Net models run through ONNX Runtime from Rust on all CPU cores, for live preview and for export (FFmpeg decodes, Rust masks each frame, FFmpeg composites with `alphamerge`). Nothing is uploaded.
* **No copies of your media.** A small loopback-only server inside the app streams files straight from disk with seeking support.

## Install

Download the installer for your system from the **Releases** page of your GitHub repository (built by the included workflow, see below):

* **Windows:** `Cutdeck_0.1.0_x64-setup.exe`. Windows SmartScreen will warn because the app isn't code-signed: click *More info → Run anyway*.
* **macOS:** the `.dmg` for Apple silicon (`aarch64`) or Intel (`x64`). The first time, right-click the app → *Open* (it isn't notarised).
* **Linux:** the `.deb` or `.AppImage`.

FFmpeg, ONNX Runtime and the AI models are bundled; nothing else to install.

## Build installers with GitHub (no local setup)

1. Create a GitHub repository and push this folder to it.
2. Push a tag: `git tag v0.1.0 && git push --tags`.
3. The **Build installers** workflow builds Windows, macOS (both chips) and Linux, runs the Rust tests, and creates a **draft release** with the installers. Publish it from the Releases page.

You can also run the workflow by hand (Actions tab → Build installers → Run workflow) to get the installers as downloadable artifacts.

## Build on your own computer

Prerequisites: [Rust](https://rustup.rs), [Node.js 20+](https://nodejs.org), and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your OS (Windows: *Microsoft C++ Build Tools* + WebView2, which Windows 10/11 already has).

**Windows (PowerShell)**

```powershell
.\scripts\fetch-deps.ps1     # FFmpeg + ONNX Runtime into src-tauri\
npm install
npm run dev                  # run in development mode
npm run build                # installers in src-tauri\target\release\bundle\
```

**macOS / Linux**

```bash
bash scripts/fetch-deps.sh
npm install
npm run dev
npm run build
```

### Tests

```bash
cd src-tauri && cargo test --lib     # renders real projects through FFmpeg, runs the AI cutout
bash tests/run-e2e.sh                # Linux: drives the real app UI end-to-end (needs xvfb, tauri-driver)
```

## Keyboard

| Key | Action |
|---|---|
| Space | Play / pause |
| S | Split at playhead |
| Delete | Delete selection |
| ← / → | Step one frame (Shift: one second) |
| Ctrl+Z / Ctrl+Shift+Z | Undo / redo |
| Ctrl+S / Ctrl+Shift+S | Save / save as |
| Ctrl+O | Open project |
| Ctrl+I | Import |
| Ctrl+E | Export |
| + / − | Zoom timeline |

## Project layout

```
src/                      editor UI (HTML/CSS/JS, fonts bundled for offline use)
src-tauri/src/
  lib.rs                  commands: dialogs, projects, proxies, export, cutout
  graph.rs                project → FFmpeg filtergraph (+ render tests)
  cutout.rs               ONNX Runtime background removal
  media.rs                loopback media server with Range support
  tools.rs                FFmpeg discovery, GPU encoder detection, probing
src-tauri/resources/
  models/                 u2netp (fast, 4.6 MB) and silueta (best, 43 MB)
  onnxruntime/            ONNX Runtime library (fetched by the script)
scripts/                  fetch-deps (FFmpeg + ONNX Runtime), prepare-models.py
tests/                    end-to-end UI test
.github/workflows/        installer builds for Windows, macOS, Linux
```

## Good to know

* **iPhone HEVC / ProRes clips:** if the system webview can't play a format, Cutdeck makes a 720p preview copy automatically. Exports always use your original file.
* **Speed and sound:** above about 4x, sped-up audio becomes noise. Choosing 8x or more mutes the clip (you can turn it back on); add music instead.
* **AI cutout speed:** "Fast" runs about 5–20 frames/s on a typical laptop CPU; "Best" is about 4x slower with cleaner hair and edges. Green screen is instant.
* **Licences:** Cutdeck's own code is yours. It bundles FFmpeg (GPL build from [BtbN](https://github.com/BtbN/FFmpeg-Builds) / [Martin Riedl](https://ffmpeg.martin-riedl.de) — source available there), [ONNX Runtime](https://github.com/microsoft/onnxruntime) (MIT), U²-Net models via [rembg](https://github.com/danielgatis/rembg) (Apache-2.0) and fonts from Fontsource (OFL). If you distribute the app, include those notices and link to the FFmpeg source.
