# Contributing to StreamCraft

Thanks for your interest. StreamCraft is a **cross-platform desktop app** + headless CLI (`streamcraft`) for downloading **non-DRM** video streams (HLS / MPEG-DASH / progressive MP4).

## Ground rules

- Follow the [Code of Conduct](CODE_OF_CONDUCT.md).
- No DRM circumvention, no decryption of protected streams, no auth bypass.
- Prefer small, focused PRs that match `ARCHITECTURE.md` module boundaries.
- Read `MEMORY.md` before non-trivial design changes.

## Setup

```bash
# Rust stable
cargo test -p streamcraft-core
cargo build -p streamcraft-cli --release   # → target/release/streamcraft
./scripts/fetch-ffmpeg.sh

cargo run -p streamcraft-app               # GUI (egui + wry)
```

CLI contract (users + agents): [`docs/cli.md`](docs/cli.md).

Legacy Swift reference only: `archive/swift/` (do not dual-maintain).

## Pull requests

1. Describe *why* (user/agent impact), not only *what*.
2. Include tests when you change Core / parsers / flix / season logic.
3. Keep public docs free of real streaming-site hostnames (use `example` placeholders).

## Licence

By contributing, you agree your contributions are licensed under the [MIT License](LICENSE).
