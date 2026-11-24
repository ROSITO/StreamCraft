# StreamCraft

Desktop app (**macOS · Windows · Linux**) to download **clear** video streams (HLS, MPEG-DASH, progressive MP4) from an embedded browser, then remux to MP4 via bundled FFmpeg.

Headless CLI **`streamcraft`** for humans and AI agents: detect, season, flix digest, download/assemble — full contract in [`docs/cli.md`](docs/cli.md).

> StreamCraft does **not** bypass DRM, captchas, or authentication.  
> Cookies / referer only replay a session you already have in the browser.

**Naming:** product = **StreamCraft**. Rust crates keep historical names `openstream-*`; the CLI **binary** is `streamcraft`.

---

## CLI — quick start (users)

### Build

```bash
# Toolchain: Rust stable
cargo build -p openstream-cli --release
# → target/release/streamcraft
```

Optional: put it on your PATH

```bash
mkdir -p ~/.local/bin
ln -sf "$(pwd)/target/release/streamcraft" ~/.local/bin/streamcraft
export PATH="$HOME/.local/bin:$PATH"
streamcraft --help
```

FFmpeg is resolved from `PATH` or bundled helpers (`./scripts/fetch-ffmpeg.sh`). Check with:

```bash
streamcraft doctor
```

### Download a known media URL

`.m3u8` / `.mpd` / `.mp4` → `download`:

```bash
streamcraft download 'https://example.com/video/master.m3u8' \
  --title 'My Video' \
  --out ~/Downloads/StreamCraft
```

JSON Lines (scripts / agents):

```bash
streamcraft download 'https://example.com/video/master.m3u8' --json --title 'My Video'
# last useful line: {"type":"completed",…,"path":"…/My Video.mp4"}
```

Default export folder: **`~/Downloads/StreamCraft/`**.  
Preferences / jobs / flix state: **`~/Library/Application Support/StreamCraft/`** (macOS).

### Other everyday commands

| Need | Command |
|------|---------|
| Page HTML → find stream | `streamcraft fetch <pageURL> --detect-only` |
| Page → download first stream | `streamcraft fetch <pageURL> --auto-download` |
| List / download a season | `streamcraft season 'Show Name' --season 1` / `--auto-download` |
| What’s new on bookmarked hubs | `streamcraft flix digest --json` |
| Job status | `streamcraft status` / `wait` / `cancel` |
| Paths & via-app | `streamcraft inbox` / `ping` |

Full options, exit codes, and agent JSON schema: **[`docs/cli.md`](docs/cli.md)**.

---

## Desktop app

```bash
./scripts/fetch-ffmpeg.sh   # sidecars (not committed)
cd apps/desktop
npm install
npm run tauri dev
```

---

## Stack

- Rust 2024 workspace: `openstream-core`, `openstream-cli` → binary **`streamcraft`**, `openstream-ffmpeg`
- Desktop: Tauri 2 + React/Vite (`apps/desktop`)
- Legacy Swift: `archive/swift/` (tag `pre-rust`)

## Layout

```
crates/openstream-core/     # parsers, download, stores, agent helpers
crates/openstream-cli/      # binary: streamcraft
crates/openstream-ffmpeg/   # sidecar resolve/spawn
apps/desktop/               # Tauri UI (productName: StreamCraft)
docs/cli.md                 # CLI user + agent contract
fixtures/                   # HLS/DASH fixtures
```

## Licence

MIT — see [LICENSE](LICENSE). FFmpeg remains an external LGPL/GPL binary invoked as a process (not linked).
