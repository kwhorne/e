#!/usr/bin/env bash
# Quick run: build debug, wrap in a .app bundle, and open it so the window
# comes to the front (macOS won't activate a bare terminal-launched GUI).
#
# Usage:  ./scripts/run.sh [path]      (path defaults to the current dir)
set -euo pipefail
cd "$(dirname "$0")/.."

TARGET="${1:-$(pwd)}"
TARGET="$(cd "$TARGET" 2>/dev/null && pwd || echo "$TARGET")"

VERSION="$(grep -E '^[[:space:]]*version[[:space:]]*=[[:space:]]*"' Cargo.toml | head -1 | sed -E 's/.*"([0-9.]+)".*/\1/')"

echo "==> building (debug)"
cargo build

APP="dist/e.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/debug/e "$APP/Contents/MacOS/e"
cp icons/e.icns "$APP/Contents/Resources/e.icns"
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>e</string>
  <key>CFBundleIdentifier</key><string>dev.e.editor</string>
  <key>CFBundleVersion</key><string>__VERSION__</string>
  <key>CFBundleShortVersionString</key><string>__VERSION__</string>
  <key>CFBundleExecutable</key><string>e</string>
  <key>CFBundleIconFile</key><string>e.icns</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
sed -i '' "s/__VERSION__/$VERSION/g" "$APP/Contents/Info.plist"

# Kill any running instance so `open` launches the freshly built binary
# (macOS `open` just focuses an existing instance instead of relaunching).
pkill -f "e.app/Contents/MacOS/e" 2>/dev/null || true
sleep 0.3

# Nudge macOS to pick up the (possibly changed) bundle icon instead of a stale
# cached one.
touch "$APP"
/usr/bin/touch "$APP/Contents/Info.plist" 2>/dev/null || true

echo "==> opening e on: $TARGET"
open -n "$APP" --args "$TARGET"
