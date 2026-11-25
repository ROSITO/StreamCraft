#!/usr/bin/env bash
# Build Craft release artefacts: streamcraft CLI + streamcraft-app + bundled FFmpeg Helpers.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
./scripts/fetch-ffmpeg.sh
cargo build --release -p streamcraft-cli -p streamcraft-app
OS_KEY="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"
case "$ARCH" in x86_64|amd64) ARCH=x86_64 ;; aarch64|arm64) ARCH=aarch64 ;; esac
case "$OS_KEY" in darwin) OS_KEY=macos ;; esac
FF="$ROOT/resources/ffmpeg/${OS_KEY}-${ARCH}"
if [[ ! -x "$FF/ffmpeg" || ! -x "$FF/ffprobe" ]]; then
  echo "error: bundled ffmpeg missing at $FF" >&2
  exit 1
fi
DEST="${1:-$ROOT/dist/craft}"
rm -rf "$DEST"
mkdir -p "$DEST/Helpers" "$DEST/bin"
cp "$FF/ffmpeg" "$FF/ffprobe" "$DEST/Helpers/"
cp "$ROOT/target/release/streamcraft" "$DEST/bin/"
cp "$ROOT/target/release/streamcraft-app" "$DEST/bin/"
# Sidecars next to binaries for find_bundled()
mkdir -p "$DEST/bin/Helpers"
cp "$FF/ffmpeg" "$FF/ffprobe" "$DEST/bin/Helpers/"
(
  cd "$DEST/bin"
  ./streamcraft doctor
)
echo "→ packaged $DEST"
