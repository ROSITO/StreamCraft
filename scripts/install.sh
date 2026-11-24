#!/usr/bin/env bash
# Install StreamCraft.app into /Applications and clear Gatekeeper quarantine.
# FFmpeg is bundled in the app (no Homebrew dependency required).
set -euo pipefail

REPO="${STREAMCRAFT_GITHUB_REPO:-ROSITO/StreamCraft}"
DEST="/Applications/StreamCraft.app"

need_cmd() { command -v "$1" >/dev/null 2>&1; }

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "StreamCraft is a macOS app." >&2
  exit 1
fi

if ! need_cmd curl || ! need_cmd python3; then
  echo "curl and python3 are required." >&2
  exit 1
fi

echo "→ Downloading latest GitHub release…"
API="https://api.github.com/repos/${REPO}/releases/latest"
ZIP_URL="$(curl -fsSL "$API" | python3 -c '
import json, sys
data = json.load(sys.stdin)
assets = data.get("assets") or []
for asset in assets:
    name = asset.get("name") or ""
    if name.endswith(".zip"):
        print(asset["browser_download_url"])
        break
else:
    sys.exit("No zip asset in the latest release.")
')"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
ZIP="$TMP/StreamCraft.zip"
curl -fL --progress-bar -o "$ZIP" "$ZIP_URL"
ditto -x -k "$ZIP" "$TMP"

APP="$(find "$TMP" -name "StreamCraft.app" -type d -maxdepth 3 | head -n 1)"
if [[ -z "$APP" ]]; then
  echo "StreamCraft.app not found in the archive." >&2
  exit 1
fi

echo "→ Copying to $DEST"
rm -rf "$DEST"
ditto "$APP" "$DEST"
xattr -cr "$DEST" 2>/dev/null || true

if [[ -x "$DEST/Contents/Resources/Helpers/ffmpeg" ]]; then
  echo "→ Bundled FFmpeg: OK"
  "$DEST/Contents/Resources/Helpers/ffmpeg" -version 2>&1 | head -1 || true
else
  echo "⚠ Bundled FFmpeg missing in this release — assembly unavailable until updated." >&2
fi

echo "StreamCraft is in Applications. You can launch:"
echo "  open -a StreamCraft"
