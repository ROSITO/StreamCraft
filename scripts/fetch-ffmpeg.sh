#!/usr/bin/env bash
# Fetch static ffmpeg/ffprobe into resources/ffmpeg/{os-arch}/
# macOS: https://ffmpeg.martin-riedl.de/
# Linux/Windows: BtbN FFmpeg-Builds, LGPL static builds
# Every archive is checked against the publisher's SHA-256 before use
# (same sources and checks as the in-app installer, streamcraft-ffmpeg/src/install.rs).
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

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

# verify FILE EXPECTED_HEX
verify() {
  local actual expected
  actual="$(sha256_of "$1")"
  expected="$(echo "$2" | tr '[:upper:]' '[:lower:]')"
  if [[ -z "$expected" || "$actual" != "$expected" ]]; then
    echo "checksum mismatch for $1 (expected '${expected}', got '${actual}')" >&2
    exit 1
  fi
  echo "  ✓ sha256 $(basename "$1")"
}

BTBN="https://github.com/BtbN/FFmpeg-Builds/releases/download/latest"
# fetch_btbn ASSET → downloads and verifies $TMP/ASSET against checksums.sha256
fetch_btbn() {
  local asset="$1"
  echo "→ $BTBN/$asset"
  curl -fL --progress-bar -o "$TMP/$asset" "$BTBN/$asset"
  curl -fsSL -o "$TMP/checksums.sha256" "$BTBN/checksums.sha256"
  verify "$TMP/$asset" "$(awk -v f="$asset" '$2 == f || $2 == "*" f {print $1}' "$TMP/checksums.sha256")"
}

fetch_macos() {
  local base="https://ffmpeg.martin-riedl.de/redirect/latest/macos"
  local channel="release"
  local arch_src
  if [[ "$ARCH" == "aarch64" ]]; then arch_src=arm64; else arch_src=amd64; fi
  for name in ffmpeg ffprobe; do
    local url="$base/$arch_src/$channel/${name}.zip"
    echo "→ $url"
    # Follow the "latest" redirect to the pinned release, whose .sha256 sits next to it.
    local final
    final="$(curl -fsSL -o "$TMP/${name}.zip" -w '%{url_effective}' "$url")"
    verify "$TMP/${name}.zip" "$(curl -fsSL "${final}.sha256" | awk '{print $1}')"
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
  local asset="ffmpeg-master-latest-linux64-lgpl.tar.xz"
  if [[ "$ARCH" == "aarch64" ]]; then
    asset="ffmpeg-master-latest-linuxarm64-lgpl.tar.xz"
  fi
  fetch_btbn "$asset"
  tar -xJf "$TMP/$asset" -C "$TMP"
  local found_ffmpeg found_ffprobe
  found_ffmpeg="$(find "$TMP" -type f -name ffmpeg | head -n 1)"
  found_ffprobe="$(find "$TMP" -type f -name ffprobe | head -n 1)"
  cp "$found_ffmpeg" "$OUT/ffmpeg"
  cp "$found_ffprobe" "$OUT/ffprobe"
  chmod +x "$OUT/ffmpeg" "$OUT/ffprobe"
}

fetch_windows() {
  local asset="ffmpeg-master-latest-win64-lgpl.zip"
  fetch_btbn "$asset"
  mkdir -p "$TMP/win"
  unzip -q "$TMP/$asset" -d "$TMP/win"
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
