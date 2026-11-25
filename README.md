# StreamCraft

<p align="center">
  <img src="apps/streamcraft/assets/icon-128.png" alt="StreamCraft" width="128" height="128" />
</p>

<p align="center">
  <strong>Desktop app</strong> (macOS · Windows · Linux) to detect and download <strong>clear</strong> video streams<br/>
  (HLS · MPEG-DASH · progressive MP4), then remux to MP4 with bundled FFmpeg.
</p>

<p align="center">
  <a href="LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/License-MIT-blue.svg" /></a>
  <a href="https://github.com/ROSITO/StreamCraft/releases"><img alt="Release" src="https://img.shields.io/github/v/release/ROSITO/StreamCraft?label=release" /></a>
  <a href="docs/cli.md"><img alt="CLI" src="https://img.shields.io/badge/CLI-streamcraft-6B21A8.svg" /></a>
</p>

> **StreamCraft does not bypass DRM, captchas, or authentication.**  
> It only downloads clear streams you can already play. Cookies / referer replay a session you already have.

---

## Features

| | |
|--|--|
| **Detect** | Capture the stream currently playing in the embedded browser (WK all-frames hooks on macOS) |
| **Download** | Parallel segments, retries, resume, assemble from `.parts` |
| **Audio / sync** | Multi-audio (FR preferred), Vidzy `gk=` unlock, optional subs |
| **Export** | Film / Series naming under `~/Downloads/StreamCraft/` |
| **CLI + MCP** | `streamcraft` — JSON Lines / MCP for agents (`docs/cli.md`) |

---

## Install

Download **v0.3.0** (Craft GUI) from [Releases](https://github.com/ROSITO/StreamCraft/releases):

| Asset | Platform |
|-------|----------|
| `StreamCraft.app-macos-aarch64.zip` | macOS Apple Silicon (GUI + CLI + FFmpeg) |
| `streamcraft-macos-aarch64` | macOS CLI only |
| `streamcraft-linux-aarch64` | Linux aarch64 CLI (glibc ≥ 2.34) |
| `streamcraft-app-linux-aarch64` | Linux aarch64 GUI (`streamcraft-app`) |
| `SHA256SUMS.txt` | Checksums |

### macOS (quick)

```bash
curl -fsSL https://raw.githubusercontent.com/ROSITO/StreamCraft/main/scripts/install.sh | bash
# → /Applications/StreamCraft.app
```

> **OpenStream** (Swift) reste sur [`ROSITO/OpenStream`](https://github.com/ROSITO/OpenStream). Ce dépôt est uniquement la stack Rust / Craft.

### Linux CLI

```bash
curl -fL -o streamcraft \
  https://github.com/ROSITO/StreamCraft/releases/latest/download/streamcraft-linux-aarch64
chmod +x streamcraft
./scripts/fetch-ffmpeg.sh   # or put ffmpeg/ffprobe in ./Helpers/
./streamcraft doctor
```

### Build from source

```bash
# Toolchain: Rust stable ≥ 1.95 (edition 2024)
./scripts/fetch-ffmpeg.sh
cargo build -p streamcraft-cli -p streamcraft-app --release
# CLI → target/release/streamcraft
# GUI → target/release/streamcraft-app

# Package macOS .app
./scripts/package-macos-app.sh   # → dist/craft/StreamCraft.app + zip

# Linux (Docker, arm64/amd64) — build + tests
./scripts/docker-linux-test.sh   # → dist/linux-*/streamcraft-linux-*
```

```bash
# optional PATH
ln -sf "$(pwd)/target/release/streamcraft" ~/.local/bin/streamcraft
streamcraft doctor
```

### CLI quick start

```bash
streamcraft download 'https://example.com/video/master.m3u8' \
  --title 'My Video' --out ~/Downloads/StreamCraft

streamcraft download 'https://example.com/video/master.m3u8' --json --title 'My Video'
# → … {"type":"completed",…,"path":"…"}
```

| Need | Command |
|------|---------|
| Page → find stream | `streamcraft fetch <url> --detect-only` |
| Page → download | `streamcraft fetch <url> --auto-download` |
| Season | `streamcraft season 'Show' --season 1` |
| What’s new | `streamcraft flix digest --json` |
| Jobs | `streamcraft status` / `wait` / `cancel` |

Full contract: [`docs/cli.md`](docs/cli.md).

---

## Layout

```
crates/streamcraft-core/      # parsers, download, stores, agent helpers
crates/streamcraft-cli/       # binary: streamcraft (+ MCP)
crates/streamcraft-ffmpeg/    # sidecar resolve / spawn
apps/streamcraft/             # Craft GUI (egui + wry) → streamcraft-app
docs/cli.md
fixtures/
archive/swift/                # legacy Swift MVP
archive/tauri-desktop/        # legacy Tauri + React shell
```

## Licence

MIT — see [LICENSE](LICENSE). FFmpeg is an external LGPL/GPL binary invoked as a process (not linked).
