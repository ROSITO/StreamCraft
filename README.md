# StreamCraft

<p align="center">
  <img src="apps/desktop/app-icon.png" alt="StreamCraft icon" width="128" height="128" />
</p>

<p align="center">
  <strong>Desktop app</strong> (macOS · Windows · Linux) to detect and download <strong>clear</strong> video streams<br/>
  (HLS · MPEG-DASH · progressive MP4) from an embedded browser, then remux to MP4 with bundled FFmpeg.
</p>

<p align="center">
  <a href="LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/License-MIT-blue.svg" /></a>
  <a href=".github/workflows/ci.yml"><img alt="CI" src="https://img.shields.io/badge/CI-GitHub%20Actions-2088FF.svg" /></a>
  <a href="CODE_OF_CONDUCT.md"><img alt="Contributor Covenant" src="https://img.shields.io/badge/Contributor%20Covenant-2.1-4baaaa.svg" /></a>
  <a href="SECURITY.md"><img alt="Security policy" src="https://img.shields.io/badge/Security-policy-informational.svg" /></a>
</p>

> **StreamCraft does not bypass DRM, captchas, or authentication.**  
> It only downloads clear streams the user can already play in the in-app browser. Session cookies/headers replay access the user already has — they are never used to break logins.

**Naming:** the product is **StreamCraft**. Internal Rust crates and the CLI binary keep the historical names `openstream-*` / `openstream-cli` for API stability.

---

## Features

| Area | What you get |
|------|----------------|
| **Detect** | NetworkObserver-style capture of the *currently playing* stream (HLS / DASH / MP4) |
| **Download** | Parallel segment download, retries, resume, assemble from `.parts` |
| **Audio / subs** | Multi-audio track picker (FR preferred), optional subtitle sidecar |
| **AV sync** | Post-export realign / drift / manual shift (Sync sheet) |
| **Export** | Film / Series / flat naming (Jellyfin-friendly) + custom templates |
| **Browser** | Popup / ad / redirect guards; bookmarks (hubs); batch URL enqueue |
| **VPN** | macOS `scutil` VPN indicator (filter Tailscale / Nord-only) |
| **CLI** | `openstream-cli` for agents — JSON Lines, season / flix helpers |

---

## Quick start

### Requirements

- **Rust** stable (edition 2024 workspace)
- **Node.js** 22+
- **Platform**: macOS 14+ recommended for the desktop WebView path; Windows / Linux via Tauri 2

### One-shot bootstrap

```bash
git clone https://github.com/ROSITO/StreamCraft.git
cd StreamCraft
chmod +x setup.sh scripts/*.sh
./setup.sh
```

### Manual

```bash
# Core + CLI
cargo test -p openstream-core
cargo build -p openstream-cli

# FFmpeg sidecars (not committed — fetch locally)
./scripts/fetch-ffmpeg.sh

# Desktop
cd apps/desktop
npm install
npm run tauri dev
```

Exports default to `~/Downloads/StreamCraft/`. Preferences / bookmarks live under Application Support / data dir `StreamCraft/`.

---

## Repository layout

```
apps/desktop/               # Tauri 2 + React/Vite UI (productName: StreamCraft)
crates/openstream-core/     # parsers, download queue, stores, agent helpers
crates/openstream-cli/      # headless openstream-cli
crates/openstream-ffmpeg/   # FFmpeg/ffprobe sidecar resolve + spawn
docs/                       # CLI contract & guides
Fixtures/                   # HLS/DASH fixtures for tests
archive/swift/              # frozen pre-Rust macOS app (reference only)
scripts/                    # fetch FFmpeg, install helpers
```

Architecture boundaries: [`ARCHITECTURE.md`](ARCHITECTURE.md).  
Product scope: [`PRD_OpenStream.md`](PRD_OpenStream.md) (historical filename; product is StreamCraft).  
Roadmap: [`ROADMAP.md`](ROADMAP.md).  
Agent memory / decisions: [`MEMORY.md`](MEMORY.md).  
CLI contract: [`docs/cli.md`](docs/cli.md).

---

## Ethics & legal

StreamCraft is for **personal / archival use of clear streams**.

**Allowed**
- Download unencrypted HLS/DASH/MP4 the user can already play
- Replay browser session cookies the user already obtained

**Forbidden (will be rejected in issues / PRs)**
- DRM circumvention (Widevine, FairPlay, PlayReady, CENC, `#EXT-X-KEY`, …)
- Decryption of protected media
- Authentication / captcha bypass
- Shipping site-specific scrape catalogs that encourage piracy

See [`SECURITY.md`](SECURITY.md) and [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md).

---

## Documentation

| Doc | Purpose |
|-----|---------|
| [CONTRIBUTING.md](CONTRIBUTING.md) | How to build, test, and open PRs |
| [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) | Community standards (Contributor Covenant 2.1) |
| [SECURITY.md](SECURITY.md) | Vulnerability reporting |
| [SUPPORT.md](SUPPORT.md) | Where to ask for help |
| [GOVERNANCE.md](GOVERNANCE.md) | Maintainer decision process |
| [docs/cli.md](docs/cli.md) | `openstream-cli` JSON Lines contract |
| [ARCHITECTURE.md](ARCHITECTURE.md) | Module boundaries |
| [CLAUDE.md](CLAUDE.md) | Agent / contributor AI instructions |

---

## Contributing

1. Read [CONTRIBUTING.md](CONTRIBUTING.md) and the CoC.
2. Open an issue before large design changes.
3. Keep PRs focused; add tests for parsers / queue / flix / season.
4. Never commit secrets, FFmpeg binaries, or real streaming-site hostnames in fixtures (use `example` placeholders).

Good first issues are labeled `good first issue` when available.

---

## Licence

[MIT](LICENSE) — Copyright © 2026 Maxime Rosito.

FFmpeg remains an **external** LGPL/GPL binary invoked as a process (not linked into StreamCraft). Run `./scripts/fetch-ffmpeg.sh` to obtain sidecars locally; they are gitignored.

---

## Acknowledgements

- Built with [Tauri](https://tauri.app/), [Tokio](https://tokio.rs/), and community crates listed in `Cargo.toml`
- Legacy Swift prototype archived under `archive/swift/` for behavioural reference
