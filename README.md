# StreamCraft

<p align="center">
  <img src="apps/desktop/src-tauri/icons/128x128.png" alt="StreamCraft" width="128" height="128" />
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
| **Detect** | Capture the stream currently playing in the embedded browser |
| **Download** | Parallel segments, retries, resume, assemble from `.parts` |
| **Audio / sync** | Multi-audio (FR preferred), optional subs, post-export A/V fixup |
| **Export** | Film / Series naming under `~/Downloads/StreamCraft/` |
| **CLI** | `streamcraft` — JSON Lines for agents (`docs/cli.md`) |

---

## Install

Download a build from [Releases](https://github.com/ROSITO/StreamCraft/releases) (macOS DMG, Linux deb/rpm/AppImage, CLI binaries).

### Build from source

```bash
# Toolchain: Rust stable, Node 22+
cargo build -p streamcraft-cli --release   # → target/release/streamcraft
./scripts/fetch-ffmpeg.sh

cd apps/desktop && npm install && npm run tauri dev
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
crates/streamcraft-cli/       # binary: streamcraft
crates/streamcraft-ffmpeg/    # sidecar resolve / spawn
apps/desktop/                 # Tauri 2 + React (productName: StreamCraft)
docs/cli.md
fixtures/
archive/swift/                # frozen pre-Rust MVP
```

## Licence

MIT — see [LICENSE](LICENSE). FFmpeg is an external LGPL/GPL binary invoked as a process (not linked).
