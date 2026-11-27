#!/usr/bin/env bash
# Build release artefacts with FFmpeg bundled: streamcraft CLI + streamcraft-app
# + ffmpeg/ffprobe in Helpers/ (found by streamcraft-ffmpeg's resolver) + notice.
# Linux → dist/craft/streamcraft-linux-<arch>.tar.gz ; Windows (Git Bash) → .zip.
# macOS uses package-macos-app.sh (StreamCraft.app).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
./scripts/fetch-ffmpeg.sh
cargo build --release -p streamcraft-cli -p streamcraft-app

ARCH="$(uname -m)"
case "$ARCH" in x86_64|amd64) ARCH=x86_64 ;; aarch64|arm64) ARCH=aarch64 ;; esac
EXE=""
case "$(uname -s)" in
  Darwin) OS_KEY=macos ;;
  Linux) OS_KEY=linux ;;
  MINGW*|MSYS*|CYGWIN*) OS_KEY=windows; EXE=".exe" ;;
  *) echo "unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac
FF="$ROOT/resources/ffmpeg/${OS_KEY}-${ARCH}"
if [[ ! -f "$FF/ffmpeg$EXE" || ! -f "$FF/ffprobe$EXE" ]]; then
  echo "error: bundled ffmpeg missing at $FF" >&2
  exit 1
fi

OUT="${1:-$ROOT/dist/craft}"
NAME="streamcraft-${OS_KEY}-${ARCH}"
DEST="$OUT/$NAME"
rm -rf "$DEST"
mkdir -p "$DEST/Helpers"
BIN="${CARGO_TARGET_DIR:-$ROOT/target}/release"
cp "$BIN/streamcraft$EXE" "$BIN/streamcraft-app$EXE" "$DEST/"
cp "$FF/ffmpeg$EXE" "$FF/ffprobe$EXE" "$DEST/Helpers/"
cp "$ROOT/resources/FFMPEG-NOTICE.txt" "$ROOT/LICENSE" "$DEST/"

# The packaged CLI must find the bundled FFmpeg on its own (no PATH fallback).
env -i HOME="$OUT" PATH=/usr/bin:/bin "$DEST/streamcraft$EXE" doctor --json | grep -q 'source=bundled' \
  || { echo "error: packaged CLI does not resolve bundled ffmpeg" >&2; exit 1; }

cd "$OUT"
if [[ "$OS_KEY" == "windows" ]]; then
  rm -f "$NAME.zip"; 7z a -tzip "$NAME.zip" "$NAME" >/dev/null
  echo "→ $OUT/$NAME.zip"
else
  rm -f "$NAME.tar.gz"; tar -czf "$NAME.tar.gz" "$NAME"
  echo "→ $OUT/$NAME.tar.gz"
fi
