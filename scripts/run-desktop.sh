#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

missing=0
for command in node pnpm cargo pkg-config; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "Missing command: $command"
    missing=1
  fi
done

if [ "$missing" -ne 0 ]; then
  echo
  echo "Install Node 22+, pnpm 10+, Rust, and the Tauri prerequisites first."
  echo "See RUN-ON-DESKTOP.md."
  exit 1
fi

for package in webkit2gtk-4.1 gtk+-3.0 javascriptcoregtk-4.1 libsoup-3.0; do
  if ! pkg-config --exists "$package"; then
    echo "Missing development package: $package"
    echo
    echo "On Debian/Ubuntu/Mint run:"
    echo "  sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \\"
    echo "    libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev"
    exit 1
  fi
done

if [ -z "${DISPLAY:-}" ] && [ -z "${WAYLAND_DISPLAY:-}" ]; then
  echo "No graphical session detected (DISPLAY and WAYLAND_DISPLAY are empty)."
  echo "Run this from a terminal inside the desktop session, not a headless SSH shell."
  exit 1
fi

pnpm install
pnpm tauri dev
