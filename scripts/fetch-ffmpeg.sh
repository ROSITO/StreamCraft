#!/usr/bin/env bash
# Fetch static ffmpeg/ffprobe into resources/ffmpeg/{os-arch}/
# macOS: https://ffmpeg.martin-riedl.de/
# Linux/Windows: https://github.com/eugeneware/ffmpeg-static style mirrors / BtbN builds
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"
case "$ARCH" in
  x86_64|amd64) ARCH=x86_64 ;;
  aarch64|arm64) ARCH=aarch64 ;;
esac
# Normalize OS key used by Rust std::env::consts::OS
case "$OS" in
  darwin) OS_KEY=macos ;;
  linux) OS_KEY=linux ;;
  mingw*|msys*|cygwin*) OS_KEY=windows ;;
  *) OS_KEY="$OS" ;;
esac
# Rust consts::OS uses "macos" not "darwin"
OUT="$ROOT/resources/ffmpeg/${OS_KEY}-${ARCH}"
# On Apple Silicon Rust reports aarch64; on Intel x86_64. Also map macos arm:
if [[ "$OS_KEY" == "macos" && "$ARCH" == "arm64" ]]; then
  OUT="$ROOT/resources/ffmpeg/macos-aarch64"
  ARCH=aarch64
fi
mkdir -p "$OUT"
FORCE=0
for arg in "$@"; do
  case "$arg" in
    --force) FORCE=1 ;;
    *) echo "Usage: $0 [--force]" >&2; exit 2 ;;
  esac
done
if [[ "$FORCE" -eq 0 && -x "$OUT/ffmpeg" && -x "$OUT/ffprobe" ]]; then
  echo "→ already present: $OUT"
  "$OUT/ffmpeg" -version | head -1 || true
  exit 0
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

fetch_macos() {
  local base="https://ffmpeg.martin-riedl.de/redirect/latest/macos"
  local channel="release"
  local arch_src
  if [[ "$ARCH" == "aarch64" ]]; then arch_src=arm64; else arch_src=x86_64; fi
  for name in ffmpeg ffprobe; do
    local url="$base/$arch_src/$channel/${name}.zip"
    echo "→ $url"
    curl -fL --progress-bar -o "$TMP/${name}.zip" "$url"
    mkdir -p "$TMP/${name}"
    ditto -x -k "$TMP/${name}.zip" "$TMP/${name}"
    local found
    found="$(find "$TMP/${name}" -type f -name "$name" | head -n 1)"
    cp "$found" "$OUT/$name"
    chmod +x "$OUT/$name"
    codesign --force --sign - "$OUT/$name" 2>/dev/null || true
  done
}

fetch_linux() {
  # BtbN shared builds (GPL/LGPL) — linux64 essentials
  local url="https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linux64-gpl.tar.xz"
  if [[ "$ARCH" == "aarch64" ]]; then
    url="https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linuxarm64-gpl.tar.xz"
  fi
  echo "→ $url"
  curl -fL --progress-bar -o "$TMP/ffmpeg.tar.xz" "$url"
  tar -xJf "$TMP/ffmpeg.tar.xz" -C "$TMP"
  local found_ffmpeg found_ffprobe
  found_ffmpeg="$(find "$TMP" -type f -name ffmpeg | head -n 1)"
  found_ffprobe="$(find "$TMP" -type f -name ffprobe | head -n 1)"
  cp "$found_ffmpeg" "$OUT/ffmpeg"
  cp "$found_ffprobe" "$OUT/ffprobe"
  chmod +x "$OUT/ffmpeg" "$OUT/ffprobe"
}

fetch_windows() {
  local url="https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip"
  echo "→ $url"
  curl -fL --progress-bar -o "$TMP/ffmpeg.zip" "$url"
  mkdir -p "$TMP/win"
  unzip -q "$TMP/ffmpeg.zip" -d "$TMP/win"
  local found_ffmpeg found_ffprobe
  found_ffmpeg="$(find "$TMP/win" -type f -name ffmpeg.exe | head -n 1)"
  found_ffprobe="$(find "$TMP/win" -type f -name ffprobe.exe | head -n 1)"
  cp "$found_ffmpeg" "$OUT/ffmpeg.exe"
  cp "$found_ffprobe" "$OUT/ffprobe.exe"
}

case "$OS_KEY" in
  macos) fetch_macos ;;
  linux) fetch_linux ;;
  windows) fetch_windows ;;
  *) echo "unsupported OS: $OS_KEY" >&2; exit 1 ;;
esac

mkdir -p "$ROOT/Vendor/ffmpeg"
date -u +"%Y-%m-%dT%H:%M:%SZ" > "$ROOT/Vendor/ffmpeg/VERSION"
echo "→ installed into $OUT"
"$OUT/ffmpeg" -version 2>/dev/null | head -1 || "$OUT/ffmpeg.exe" -version 2>/dev/null | head -1 || true
