#!/usr/bin/env bash
# Build a macOS .app bundle for `e` with the app icon.
set -euo pipefail

cd "$(dirname "$0")/.."

VERSION="$(grep -E '^[[:space:]]*version[[:space:]]*=[[:space:]]*"' Cargo.toml | head -1 | sed -E 's/.*"([0-9.]+)".*/\1/')"

echo "==> building release binary (e $VERSION)"
cargo build --release

APP="dist/e.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

cp target/release/e "$APP/Contents/MacOS/e"
cp icons/e.icns "$APP/Contents/Resources/e.icns"
cat > "$APP/Contents/Resources/Credits.html" <<'CREDITS'
<!DOCTYPE html><html><head><meta charset="utf-8"><style>
body{font-family:-apple-system,sans-serif;font-size:12px;color:#333;text-align:center;margin:8px}
a{color:#2563eb;text-decoration:none}
</style></head><body>
<p>The editor for the rest of us</p>
<p><a href="https://elyracode.com/e">elyracode.com/e</a> Â·
<a href="https://elyracode.com/docs/e">docs</a> Â·
<a href="https://github.com/kwhorne/e">github.com/kwhorne/e</a></p>
</body></html>
CREDITS

cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>            <string>e</string>
  <key>CFBundleDisplayName</key>     <string>e</string>
  <key>CFBundleIdentifier</key>      <string>dev.e.editor</string>
  <key>CFBundleVersion</key>         <string>__VERSION__</string>
  <key>CFBundleShortVersionString</key><string>__VERSION__</string>
  <key>CFBundlePackageType</key>     <string>APPL</string>
  <key>CFBundleExecutable</key>      <string>e</string>
  <key>CFBundleIconFile</key>        <string>e.icns</string>
  <key>NSHighResolutionCapable</key> <true/>
  <key>NSHumanReadableCopyright</key> <string>The editor for the rest of us · © 2026 Knut W. Horne</string>
  <key>LSMinimumSystemVersion</key>  <string>11.0</string>
</dict>
</plist>
PLIST

# Substitute the version read from Cargo.toml.
sed -i '' "s/__VERSION__/$VERSION/g" "$APP/Contents/Info.plist"

# Ad-hoc sign so macOS treats it as a stable, consistent app (no Developer ID).
if command -v codesign >/dev/null 2>&1; then
  codesign --force --deep --sign - "$APP" 2>/dev/null || true
fi

echo "==> built $APP (e $VERSION)"
