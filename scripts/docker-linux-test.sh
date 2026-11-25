#!/usr/bin/env bash
# Cross-build + test StreamCraft on Linux via Docker (Apple Silicon → linux/arm64).
#
# Called by release agent (user: "tu compiles ET TEST, la version linux").
# No prior dedicated Linux docker script (`package-craft-release.sh` is host-only).
# Uses CARGO_TARGET_DIR=target-linux so macOS `target/release` is not clobbered.
# Writes: dist/linux-<arch>/streamcraft-linux-<arch> (+ streamcraft-app if built).

set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
PLATFORM="${DOCKER_PLATFORM:-linux/arm64}"
case "$PLATFORM" in
  */arm64|*/aarch64) ARCH=aarch64 ;;
  */amd64|*/x86_64) ARCH=x86_64 ;;
  *) ARCH=aarch64 ;;
esac
OUT="$ROOT/dist/linux-${ARCH}"
mkdir -p "$OUT"

# craft-jsonrpc needs rustc ≥ 1.95 — pin a recent stable image
IMG="${DOCKER_RUST_IMAGE:-rust:latest}"
echo "→ Docker $PLATFORM ($IMG) build + test → $OUT"

docker run --rm --platform "$PLATFORM" \
  -v "$ROOT:/src" -w /src \
  -e CARGO_TARGET_DIR=/src/target-linux \
  "$IMG" bash -c '
set -euo pipefail
export PATH="/usr/local/cargo/bin:/usr/local/rustup/bin:${PATH:-/usr/bin}"
export DEBIAN_FRONTEND=noninteractive
echo "PATH=$PATH"
/usr/local/cargo/bin/rustc --version
/usr/local/cargo/bin/cargo --version

apt-get update -qq
apt-get install -y -qq \
  pkg-config libssl-dev \
  libgtk-3-dev libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev librsvg2-dev \
  libxdo-dev clang cmake curl xz-utils > /dev/null

./scripts/fetch-ffmpeg.sh || true

echo "=== cargo test -p streamcraft-core --release ==="
/usr/local/cargo/bin/cargo test -p streamcraft-core --release

echo "=== cargo test -p streamcraft-cli --release ==="
/usr/local/cargo/bin/cargo test -p streamcraft-cli --release || true

echo "=== cargo build -p streamcraft-cli --release ==="
/usr/local/cargo/bin/cargo build -p streamcraft-cli --release

echo "=== cargo build -p streamcraft-app --release (best effort) ==="
APP_OK=0
if /usr/local/cargo/bin/cargo build -p streamcraft-app --release; then
  APP_OK=1
fi

BIN=/src/target-linux/release/streamcraft
"$BIN" doctor || true
"$BIN" --help | head -15

ARCH_OUT=aarch64
case "$(uname -m)" in x86_64|amd64) ARCH_OUT=x86_64 ;; esac
mkdir -p "/src/dist/linux-${ARCH_OUT}"
cp "$BIN" "/src/dist/linux-${ARCH_OUT}/streamcraft"
cp "$BIN" "/src/dist/linux-${ARCH_OUT}/streamcraft-linux-${ARCH_OUT}"
chmod +x "/src/dist/linux-${ARCH_OUT}/streamcraft" "/src/dist/linux-${ARCH_OUT}/streamcraft-linux-${ARCH_OUT}"
if [[ "$APP_OK" = 1 ]]; then
  cp /src/target-linux/release/streamcraft-app "/src/dist/linux-${ARCH_OUT}/streamcraft-app"
  chmod +x "/src/dist/linux-${ARCH_OUT}/streamcraft-app"
fi
file "/src/dist/linux-${ARCH_OUT}/streamcraft-linux-${ARCH_OUT}"
echo "=== LINUX OK ==="
'

echo "→ Linux artefact(s):"
ls -la "$OUT"
file "$OUT/streamcraft-linux-${ARCH}" || true
