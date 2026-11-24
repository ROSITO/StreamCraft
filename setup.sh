#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"

echo "==> StreamCraft setup"
command -v rustc >/dev/null || { echo "Install Rust from https://rustup.rs"; exit 1; }
command -v node >/dev/null || { echo "Install Node.js 22+"; exit 1; }

echo "==> cargo fetch / test core"
cargo fetch
cargo test -p openstream-core --lib

if [[ -x ./scripts/fetch-ffmpeg.sh ]]; then
  echo "==> fetching FFmpeg sidecars"
  ./scripts/fetch-ffmpeg.sh || echo "warn: FFmpeg fetch failed — desktop remux needs ffmpeg/ffprobe"
fi

echo "==> desktop npm install"
( cd apps/desktop && npm install )

echo ""
echo "Done. Run desktop with:"
echo "  cd apps/desktop && npm run tauri dev"
echo "CLI:"
echo "  cargo run -p openstream-cli -- --help"
