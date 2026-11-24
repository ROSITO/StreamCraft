#!/usr/bin/env bash
# Compat wrapper — prefers multi-OS script, mirrors into legacy Helpers path on macOS.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
"$ROOT/scripts/fetch-ffmpeg.sh" "$@"
OS_KEY=macos
ARCH="$(uname -m)"
case "$ARCH" in
  arm64|aarch64) DEST_ARCH=aarch64 ;;
  *) DEST_ARCH=x86_64 ;;
esac
SRC="$ROOT/resources/ffmpeg/${OS_KEY}-${DEST_ARCH}"
LEGACY="$ROOT/resources/Helpers"
# Also mirror for archived Swift path if present
SWIFT_HELPERS="$ROOT/archive/swift/OpenStream/Resources/Helpers"
mkdir -p "$LEGACY"
if [[ -x "$SRC/ffmpeg" ]]; then
  ln -sf "$SRC/ffmpeg" "$LEGACY/ffmpeg"
  ln -sf "$SRC/ffprobe" "$LEGACY/ffprobe"
  if [[ -d "$(dirname "$SWIFT_HELPERS")" ]]; then
    mkdir -p "$SWIFT_HELPERS"
    ln -sf "$SRC/ffmpeg" "$SWIFT_HELPERS/ffmpeg"
    ln -sf "$SRC/ffprobe" "$SWIFT_HELPERS/ffprobe"
  fi
fi
