#!/usr/bin/env bash
# Quick run: build debug, wrap in a .app bundle, and open it so the window
# comes to the front (macOS won't activate a bare terminal-launched GUI).
#
# Usage:  ./scripts/run.sh [path]      (path defaults to the current dir)
set -euo pipefail
cd "$(dirname "$0")/.."

TARGET="${1:-$(pwd)}"
TARGET="$(cd "$TARGET" 2>/dev/null && pwd || echo "$TARGET")"

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
  <key>CFBundleVersion</key><string>0.1.0</string>
  <key>CFBundleExecutable</key><string>e</string>
  <key>CFBundleIconFile</key><string>e.icns</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST

# Kill any running instance so `open` launches the freshly built binary
# (macOS `open` just focuses an existing instance instead of relaunching).
pkill -f "e.app/Contents/MacOS/e" 2>/dev/null || true
sleep 0.3

echo "==> opening e on: $TARGET"
open -n "$APP" --args "$TARGET"
