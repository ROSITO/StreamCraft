#!/usr/bin/env bash
# Build StreamCraft.app (Craft egui) + zip for GitHub Releases.
#
# Called by: release pipeline / agent when packaging macOS (user: "tu compile
# la release macOS"). Extends `package-craft-release.sh` which only copies bare
# binaries — this script produces StreamCraft.app + Info.plist + AppIcon.icns +
# zip matching prior release asset names (StreamCraft.app-macos-aarch64.zip).
# Reads: target/release/{streamcraft,streamcraft-app}, resources/ffmpeg/macos-*,
# apps/streamcraft/assets/AppIcon.icns. Writes: dist/craft/StreamCraft.app,
# dist/craft/StreamCraft.app-macos-*.zip, dist/craft/streamcraft-macos-*.

set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
VERSION="${VERSION:-$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')}"
ARCH="$(uname -m)"
case "$ARCH" in arm64|aarch64) ARCH=aarch64 ;; x86_64|amd64) ARCH=x86_64 ;; esac

./scripts/fetch-ffmpeg.sh
cargo build --release -p streamcraft-cli -p streamcraft-app

FF="$ROOT/resources/ffmpeg/macos-${ARCH}"
DEST="${1:-$ROOT/dist/craft}"
APP="$DEST/StreamCraft.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$APP/Contents/Helpers"

cp "$ROOT/target/release/streamcraft-app" "$APP/Contents/MacOS/streamcraft-app"
cp "$ROOT/target/release/streamcraft" "$APP/Contents/MacOS/streamcraft"
cp "$FF/ffmpeg" "$FF/ffprobe" "$APP/Contents/Helpers/"
chmod +x "$APP/Contents/MacOS/"* "$APP/Contents/Helpers/"*

if [[ -f "$ROOT/apps/streamcraft/assets/AppIcon.icns" ]]; then
  cp "$ROOT/apps/streamcraft/assets/AppIcon.icns" "$APP/Contents/Resources/AppIcon.icns"
fi

cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleExecutable</key><string>streamcraft-app</string>
  <key>CFBundleIdentifier</key><string>app.streamcraft.desktop</string>
  <key>CFBundleName</key><string>StreamCraft</string>
  <key>CFBundleDisplayName</key><string>StreamCraft</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
EOF
echo -n 'APPL????' > "$APP/Contents/PkgInfo"

codesign --force --deep -s - "$APP" 2>/dev/null || true

mkdir -p "$DEST/bin" "$DEST/Helpers"
cp "$ROOT/target/release/streamcraft" "$DEST/bin/"
cp "$ROOT/target/release/streamcraft-app" "$DEST/bin/"
cp "$FF/ffmpeg" "$FF/ffprobe" "$DEST/Helpers/"
mkdir -p "$DEST/bin/Helpers"
cp "$FF/ffmpeg" "$FF/ffprobe" "$DEST/bin/Helpers/"

ZIP="$DEST/StreamCraft.app-macos-${ARCH}.zip"
rm -f "$ZIP"
ditto -c -k --sequesterRsrc --keepParent "$APP" "$ZIP"
cp "$DEST/bin/streamcraft" "$DEST/streamcraft-macos-${ARCH}"

(
  cd "$DEST/bin"
  ./streamcraft doctor
)

echo "→ macOS app: $APP"
echo "→ zip: $ZIP"
echo "→ cli: $DEST/streamcraft-macos-${ARCH}"
