<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" alt="Fuji Culler" width="128" />
</p>

# Fuji Culler

A fast, focused desktop app for culling and importing photos and videos from Fuji cameras. Connect your camera, rate your keepers, and import — all without leaving the app.

- **Project website:** [wolves.ink/projects/fuji](https://wolves.ink/projects/fuji)
- **Wolves:** [wolves.ink](https://wolves.ink)

---

## Install

Download the latest `fuji.dmg` from [GitHub Releases](https://github.com/wolvesdotink/fuji/releases/latest), open it, and drag Fuji Culler to Applications.

The app is signed and notarized, so macOS will open it without warnings on first launch. Existing installs receive new versions automatically via the in-app updater.

**Apple Silicon only.** Intel Macs are not currently supported.

---

## Features

### Camera Import Workflow
- **Auto-detection** — Plug in your Fuji camera (USB mass storage or PTP) and Fuji Culler detects it instantly
- **Star-based culling** — Rate photos 1–5 with a single keypress; ratings auto-advance to the next unreviewed image
- **Smart import logic** — Ratings 1–3 import HEIF only; ratings 4–5 import both HEIF and the original RAW (.RAF) file
- **Video import and playback** — Import MOV, MP4, M4V, and AVI clips, then browse, rate, search, and play them in the library
- **Respects camera ratings** — Reads star ratings already set on-camera from HIF XMP metadata
- **Side-by-side compare** — Mark two or more images with `M` and open them in compare view with `C` to pick the best shot
- **PTP support** — Fully supports Fuji cameras connected via PTP (not just mass storage), including catalog, preview download, and delete after import
- **Delete after import** — Remove imported or skipped files from the camera card after a session

### Library Browser
- **Browse imported media** — View and re-rate any folder of previously imported photos and videos
- **Sort by date or stars** — Multiple sort orders to find photos quickly
- **AI-powered semantic search** — Type natural language queries like "golden hour landscape" or "portrait with bokeh" and find matching photos using a local CLIP model (no internet required after first download)
- **CLIP index** — Automatically built in the background after each library load; the model runs entirely on-device

### General
- **Thumbnail generation** — Fast RAF → JPEG thumbnails generated from raw files and cached locally
- **Progress tracking** — Real-time import progress with bytes copied, file count, and phase (copying, verifying, complete)
- **Keyboard-first** — Every core action has a keyboard shortcut; no mouse required for the culling workflow
- **Auto-updater** — Built-in update check and install via GitHub Releases

---

## Use Cases

- **Fuji shooters who cull on import** — Decide what to keep before files ever land on your drive
- **Dual-format RAW+JPEG shooters** — Let ratings decide automatically: keep the JPEG for keepers, grab the RAW for your best shots
- **High-volume sessions** — Keyboard shortcuts and auto-advance let you move through hundreds of images in minutes
- **Finding photos by memory** — Search your library with plain English instead of digging through folders or date ranges

---

## Keyboard Shortcuts

### Camera Mode

| Key | Action |
|-----|--------|
| `←` / `→` | Previous / Next image |
| `1`–`5` | Rate and advance to next unreviewed |
| `0` | Clear rating and advance |
| `Space` | Jump to next unreviewed image |
| `G` | Toggle grid / single view |
| `M` | Mark / unmark image for compare |
| `C` | Open compare view (requires 2+ marked) |
| `Esc` | Exit compare mode |

### Library Mode

| Key | Action |
|-----|--------|
| `←` / `→` | Previous / Next image |
| `1`–`5` | Set star rating |
| `0` | Clear rating |
| `G` | Toggle grid / single view |
| `/` | Focus search input |
| `Esc` | Return to grid or clear search |

---

## Technical Details

### Stack

| Layer | Technology |
|-------|-----------|
| Framework | [Tauri 2](https://tauri.app) (Rust backend + WebView frontend) |
| Frontend | [Vue 3](https://vuejs.org) + [Pinia](https://pinia.vuejs.org) + TypeScript |
| Styling | [Tailwind CSS v4](https://tailwindcss.com) |
| Build | [Vite 6](https://vitejs.dev) |
| Package manager | [pnpm](https://pnpm.io) |

### Architecture

- **Rust backend** handles all file I/O, thumbnail generation, XMP metadata reads/writes, PTP camera communication (via an external `ptp-bridge` binary), and CLIP model inference
- **Vue frontend** is a single-page app communicating with the Rust layer via Tauri's `invoke` IPC and streaming progress via `Channel`
- **Pinia stores** (`gallery`, `library`, `app`) hold all application state; no server, no database
- **Thumbnails and CLIP index** are cached at `~/.cache/fuji-culler/` and persist between sessions

### PTP Support

Fuji cameras connected over PTP are handled by a bundled `ptp-bridge` binary (macOS arm64). The bridge wraps the platform's PTP/MTP stack and exposes a simple CLI that the Rust backend calls for listing, downloading, and deleting files.

### CLIP Search

The semantic search feature uses a quantized CLIP model downloaded on first use to `~/.cache/fuji-culler/models/`. The model runs entirely locally — no images or queries leave your machine. The index (`clip-index.bin`) is rebuilt from thumbnails each time you open a folder.

### System Requirements

- macOS 11 (Big Sur) or later
- Apple Silicon Mac

---

## Development

```bash
# Install dependencies
pnpm install

# Run in development mode (hot reload)
pnpm tauri dev

# Build a release bundle (.app + .dmg)
pnpm tauri build

# Bump version (patch / minor / major)
pnpm bump:patch
```

Requires [Rust](https://rustup.rs) and the [Tauri CLI prerequisites](https://tauri.app/start/prerequisites/).

### DMG installer background

The DMG window background is rendered from `src-tauri/assets/dmg-background.svg` (the source of truth) into `src-tauri/assets/dmg-background.png`, which Tauri reads at bundle time. The PNG is checked in so CI doesn't need any image tooling — only edit the SVG and regenerate when changing the design:

```bash
brew install librsvg          # one-time, only if you don't have rsvg-convert
./scripts/build-dmg-background.sh
```

Window size and icon positions live alongside the background path in `src-tauri/tauri.conf.json` under `bundle.macOS.dmg`.

---

## License

MIT License

Copyright (c) 2026 Wolves

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
