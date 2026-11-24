# Contributing to StreamCraft

Thanks for helping. StreamCraft is a cross-platform desktop app + headless CLI for downloading **non-DRM** video streams (HLS / MPEG-DASH / progressive MP4).

## Ground rules

- Follow the [Code of Conduct](CODE_OF_CONDUCT.md).
- **No** DRM circumvention, decryption of protected streams, or auth bypass — such PRs / issues will be closed.
- Prefer small, focused PRs that respect [`ARCHITECTURE.md`](ARCHITECTURE.md) module boundaries (UI → Core → Browser → Network → Detector → Parser → Download → FFmpeg).
- Read [`MEMORY.md`](MEMORY.md) before non-trivial design changes.
- Product name is **StreamCraft**; crate/CLI names remain `openstream-*` unless a dedicated rename RFC is accepted.

## Development setup

```bash
# Toolchain
rustup update stable
node -v   # 22+

git clone https://github.com/ROSITO/StreamCraft.git
cd StreamCraft
./setup.sh

# Day-to-day
cargo test -p openstream-core
cargo build -p openstream-cli
cd apps/desktop && npm install && npm run tauri dev
```

FFmpeg sidecars: `./scripts/fetch-ffmpeg.sh` (or `./scripts/fetch-bundled-ffmpeg.sh`). Binaries are **not** committed.

### Tests

```bash
cargo test -p openstream-core
cargo test -p openstream-ffmpeg
# Desktop: prefer manual smoke (detect → download → Sync) on clear fixtures / public test streams
```

CLI agent contract: [`docs/cli.md`](docs/cli.md).

## Pull requests

1. Describe *why* (user / agent impact), not only *what*.
2. Include tests when you change Core / parsers / download / flix / season.
3. Keep public docs free of real streaming-site hostnames (use `example` placeholders).
4. Fill the PR template; link related issues.
5. Ensure CI is green (`.github/workflows/ci.yml`).

## Issue tracker

- Bugs → bug report template
- Features → feature request template  
- Security → [SECURITY.md](SECURITY.md) (private advisory preferred)

## Licence

By contributing, you agree your contributions are licensed under the [MIT License](LICENSE).
