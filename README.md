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

Download the latest installers from the GitHub **Releases** page. The release workflow uploads these stable names (the website links to `releases/latest/download/<name>`):

| System | File |
| --- | --- |
| macOS Apple Silicon | `Cutdeck-macos-apple-silicon.dmg` |
| macOS Intel | `Cutdeck-macos-intel.dmg` |
| Linux | `Cutdeck-linux-x86_64.AppImage` and `Cutdeck-linux-amd64.deb` |
| Windows | `Cutdeck-windows-x64-setup.exe` and `Cutdeck-windows-x64.msi` |

FFmpeg, ONNX Runtime, the caption engine and the AI models are bundled. Speech models download on first use and stay on the computer.

The builds are **not code-signed or notarized**.

* **Windows:** SmartScreen says the installer isn't recognized. Choose *More info → Run anyway*.
* **macOS (Gatekeeper):** right-click Cutdeck → *Open* → *Open*. If it still will not open:

  ```bash
  xattr -dr com.apple.quarantine /Applications/Cutdeck.app
  ```

  Use the Apple Silicon disk image on M1/M2/M3/M4 and the Intel disk image on older Macs. macOS 11 or later.
* **Linux:** `chmod +x Cutdeck-linux-x86_64.AppImage` and run it, or `sudo apt install ./Cutdeck-linux-amd64.deb` (the package pulls in WebKitGTK).

### Signing and notarizing macOS later

Unsigned is enough to ship. Notarization needs an Apple Developer account. When you have one, add these repository secrets and Tauri will sign during `tauri build` if the environment variables are set on the macOS jobs:

| Secret | What it is |
| --- | --- |
| `APPLE_CERTIFICATE` | Base64 of the Developer ID Application `.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | Password for that `.p12` |
| `APPLE_SIGNING_IDENTITY` | Developer ID Application name |
| `APPLE_ID` | Apple ID email |
| `APPLE_PASSWORD` | App-specific password |
| `APPLE_TEAM_ID` | Team ID |

Until those are set, leave them unset. An empty identity makes the build try to sign and fail. The workflow does not pass them.

## Cut a release

1. Match `version` in `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` and `package.json` if you want the tag to agree with the repo (the workflow also writes the tag version into those files on the runner).
2. Tag and push: `git tag v0.1.0 && git push origin v0.1.0`.
3. The **Release** workflow (`.github/workflows/release.yml`) builds Windows, macOS Apple Silicon, macOS Intel (cross-compiled on the Apple Silicon runner) and Linux, runs the Rust tests, and publishes a GitHub Release with the installers above. `releases/latest` only sees a published release, which is what the website download buttons use.
4. To build without publishing, open Actions → Release → Run workflow. The installers are artifacts on that run.

## Website (GitHub Pages)

The public site lives in `website/`. The home page is a grid of apps from `website/data/apps.json`. Cutdeck’s product page is `website/cutdeck/index.html`.

**Support link:** edit `supportUrl` in `website/data/site.json`. It starts as `https://buymeacoffee.com/REPLACE_ME`. The amber “Buy me a coffee” button reads that file. While the URL contains `REPLACE_ME`, the site shows a placeholder note.

**Add another app:** append an entry to `website/data/apps.json` and add `website/<id>/index.html`.

**Turn on GitHub Pages** (once):

1. Merge this to `master`.
2. On GitHub: **Settings → Pages → Build and deployment → Source: GitHub Actions**.
3. The **GitHub Pages** workflow (`.github/workflows/pages.yml`) deploys `website/` on pushes to `master` that touch the site. The site is `https://manoj7019.github.io/cutdeck-desktop/`.

Canonical URLs in the HTML use that address. If you add a custom domain, update them and `pagesUrl` in `website/data/site.json`.

## Build on your own computer

Prerequisites: [Rust](https://rustup.rs), [Node.js 20+](https://nodejs.org), and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your OS (Windows: *Microsoft C++ Build Tools* + WebView2, which Windows 10/11 already has). macOS also needs CMake (`brew install cmake`) so the caption engine can be built. Linux needs the WebKitGTK packages below.

**Windows (PowerShell)**

```powershell
.\scripts\fetch-deps.ps1     # FFmpeg + ONNX Runtime into src-tauri\
npm install
npm run dev                  # run in development mode
npm run build                # installers in src-tauri\target\release\bundle\
```

**macOS / Linux**

```bash
# Linux (Debian/Ubuntu), once:
sudo apt-get install -y libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf libssl-dev libfuse2 file build-essential curl wget xz-utils unzip pkg-config libxdo-dev
# Ubuntu 24.04 names the FUSE library libfuse2t64 if libfuse2 is not a package.

bash scripts/fetch-deps.sh
npm install
npm run dev
APPIMAGE_EXTRACT_AND_RUN=1 npm run build
```

`scripts/fetch-deps.sh` takes an optional target triple (`aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`). It refuses an FFmpeg build that lacks vidstab (stabilization) or libass (burned-in captions). On macOS it also requires VideoToolbox. Apple Silicon FFmpeg comes from OSXExperts; Intel FFmpeg comes from evermeet.cx, with an OSXExperts fallback. whisper.cpp is the official Linux build, and is compiled from the same tag on macOS.

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
scripts/                  fetch-deps (FFmpeg + ONNX Runtime + whisper.cpp), prepare-models.py
tests/                    end-to-end UI test
website/                  static site (home grid + Cutdeck page) for GitHub Pages
.github/workflows/        release.yml (installers), pages.yml (the site)
```

## Good to know

* **iPhone HEVC / ProRes clips:** if the system webview can't play a format, Cutdeck makes a 720p preview copy automatically. Exports always use your original file.
* **Speed and sound:** above about 4x, sped-up audio becomes noise. Choosing 8x or more mutes the clip (you can turn it back on); add music instead.
* **AI cutout speed:** "Fast" runs about 5–20 frames/s on a typical laptop CPU; "Best" is about 4x slower with cleaner hair and edges. Green screen is instant.
* **Licences:** Cutdeck's own code is yours. It bundles FFmpeg GPL builds — [BtbN](https://github.com/BtbN/FFmpeg-Builds) on Windows and Linux, [OSXExperts](https://www.osxexperts.net/) on Apple Silicon, [evermeet.cx](https://evermeet.cx/ffmpeg/) on Intel Macs (source is the matching FFmpeg tree) — plus [whisper.cpp](https://github.com/ggml-org/whisper.cpp) (MIT), [ONNX Runtime](https://github.com/microsoft/onnxruntime) (MIT), U²-Net models via [rembg](https://github.com/danielgatis/rembg) (Apache-2.0) and fonts from Fontsource (OFL). If you distribute the app, include those notices and link to the FFmpeg source.
