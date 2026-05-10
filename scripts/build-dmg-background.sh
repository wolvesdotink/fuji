#!/usr/bin/env bash
# build-dmg-background.sh — render src-tauri/assets/dmg-background.svg to PNG.
#
# Usage: ./scripts/build-dmg-background.sh
#
# The SVG is the source of truth; the PNG is what tauri.conf.json points the
# DMG bundler at. We render at 2× the window size (1320×800 px) and stamp the
# PNG with 144 DPI so macOS Finder treats it as a Retina (@2x) asset — i.e.
# "1320×800 pixels at 144 DPI = 660×400 points." Without the DPI stamp, Finder
# would render the PNG at 1:1 and the image would overflow the window.
#
# Requires librsvg (rsvg-convert): brew install librsvg
# sips ships with macOS; no install needed.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
SVG="$ROOT/src-tauri/assets/dmg-background.svg"
PNG="$ROOT/src-tauri/assets/dmg-background.png"

if ! command -v rsvg-convert >/dev/null 2>&1; then
  echo "Error: rsvg-convert not found." >&2
  echo "Install with: brew install librsvg" >&2
  exit 1
fi

if [[ ! -f "$SVG" ]]; then
  echo "Error: SVG source not found at $SVG" >&2
  exit 1
fi

rsvg-convert -w 1320 -h 800 "$SVG" -o "$PNG"
sips -s dpiHeight 144 -s dpiWidth 144 "$PNG" --out "$PNG" >/dev/null

echo "Wrote $PNG ($(file -b "$PNG"))"
