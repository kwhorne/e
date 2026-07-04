#!/usr/bin/env bash
#
# Build a macOS .dmg installer for `e`.
#
#   ./scripts/bundle-dmg.sh             # host architecture
#   ./scripts/bundle-dmg.sh --universal # universal (arm64 + x86_64)
#
# Produces dist/e-<version>[-arch].dmg containing e.app and an Applications
# symlink, so users just drag the app into Applications.
#
# Code signing / notarization (optional, for distribution without Gatekeeper
# warnings) is described in docs/installation.md. By default the app is ad-hoc
# signed; set CODESIGN_IDENTITY to a "Developer ID Application: …" identity to
# sign properly.

set -euo pipefail
cd "$(dirname "$0")/.."

# Shared signing/notarization helpers. Default to the local keychain profile.
source "$(dirname "$0")/sign.sh"
: "${NOTARY_PROFILE:=e-notary}"

VERSION="$(grep -E '^[[:space:]]*version[[:space:]]*=[[:space:]]*"' Cargo.toml | head -1 | sed -E 's/.*"([0-9.]+)".*/\1/')"
UNIVERSAL=0
[[ "${1:-}" == "--universal" ]] && UNIVERSAL=1

APP="dist/e.app"

# --- 1. build the binary ---------------------------------------------------
if [[ "$UNIVERSAL" == "1" ]]; then
  echo "==> building universal binary (arm64 + x86_64)"
  rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null 2>&1 || true
  cargo build --release --bin e --target aarch64-apple-darwin
  cargo build --release --bin e --target x86_64-apple-darwin
  mkdir -p target/release
  lipo -create -output target/release/e \
    target/aarch64-apple-darwin/release/e \
    target/x86_64-apple-darwin/release/e
  ARCH_SUFFIX="-universal"
else
  cargo build --release --bin e
  ARCH_SUFFIX=""
fi

# --- 2. assemble the .app bundle ------------------------------------------
echo "==> assembling $APP"
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

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>            <string>e</string>
  <key>CFBundleDisplayName</key>     <string>e</string>
  <key>CFBundleIdentifier</key>      <string>dev.e.editor</string>
  <key>CFBundleVersion</key>         <string>$VERSION</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundlePackageType</key>     <string>APPL</string>
  <key>CFBundleExecutable</key>      <string>e</string>
  <key>CFBundleIconFile</key>        <string>e.icns</string>
  <key>NSHighResolutionCapable</key> <true/>
  <key>NSHumanReadableCopyright</key> <string>The editor for the rest of us · © 2026 Knut W. Horne</string>
  <key>LSMinimumSystemVersion</key>  <string>11.0</string>

  <!-- e is a text editor that can open any file, so macOS offers Open With
       for .sql, .env, .log and anything else instead of "e cannot open files
       of this type". Rank Alternate so default associations are untouched. -->
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key>       <string>Text Document</string>
      <key>CFBundleTypeRole</key>       <string>Editor</string>
      <key>LSHandlerRank</key>          <string>Alternate</string>
      <key>LSItemContentTypes</key>
      <array>
        <string>public.text</string>
        <string>public.plain-text</string>
        <string>public.source-code</string>
        <string>public.data</string>
      </array>
      <key>CFBundleTypeExtensions</key>
      <array><string>*</string></array>
      <key>CFBundleTypeOSTypes</key>
      <array><string>****</string></array>
    </dict>
  </array>
</dict>
</plist>
PLIST

# --- 3. sign ---------------------------------------------------------------
IDENTITY="$(detect_identity)"; IDENTITY="${IDENTITY:--}"
sign_app "$APP" "$IDENTITY"

# In CI the whole target dir (10+ GB: per-arch builds plus the universal
# release) is dead weight once the signed app is staged under dist/ — the
# universal binary already lives inside the app bundle. Free all of it so
# hdiutil has room to build the DMG (the runner kept hitting "No space left on
# device", even after only the per-arch dirs were removed).
if [[ -n "${CI:-}" ]]; then
  echo "==> CI: freeing build caches before building the DMG"
  # The signed app is already staged under dist/, and cargo/rustup aren't needed
  # again in this script, so drop everything heavy: the target dir (10+ GB) plus
  # the cargo registry/git checkouts and rustup toolchains. The runner kept
  # hitting "No space left on device" on hdiutil even after only target/ went.
  rm -rf target || true
  rm -rf "${CARGO_HOME:-$HOME/.cargo}/registry" "${CARGO_HOME:-$HOME/.cargo}/git" || true
  rm -rf "${RUSTUP_HOME:-$HOME/.rustup}/toolchains" || true
  # hdiutil and mktemp write scratch to $TMPDIR. On the CI runners that points at
  # a tiny volume even though the workspace has ~90 GB free, which is why the DMG
  # build kept failing with "No space left on device" while `df /` looked fine.
  # Pin TMPDIR into the workspace so both the staging copy and hdiutil's scratch
  # land on the roomy volume.
  export TMPDIR="$PWD/.dmg-tmp"
  rm -rf "$TMPDIR"
  mkdir -p "$TMPDIR"
  df -h / . "$TMPDIR" || true
fi

# --- 4. build the DMG ------------------------------------------------------
DMG="dist/e-${VERSION}${ARCH_SUFFIX}.dmg"
echo "==> building $DMG"
rm -f "$DMG"

if command -v create-dmg >/dev/null 2>&1; then
  # Prettier layout if create-dmg is installed (brew install create-dmg).
  create-dmg \
    --volname "e $VERSION" \
    --window-size 540 380 \
    --icon-size 110 \
    --icon "e.app" 140 180 \
    --app-drop-link 400 180 \
    "$DMG" "$APP" >/dev/null
else
  # Fallback: a plain DMG with an Applications symlink, using built-in hdiutil.
  STAGING="$(mktemp -d)"
  cp -R "$APP" "$STAGING/"
  ln -s /Applications "$STAGING/Applications"
  hdiutil create -volname "e $VERSION" -srcfolder "$STAGING" \
    -ov -format UDZO "$DMG" >/dev/null
  rm -rf "$STAGING"
fi

# --- 5. sign + notarize + staple the DMG -----------------------------------
if [[ "$IDENTITY" != "-" ]]; then
  sign_dmg "$DMG" "$IDENTITY"
  if notarize_staple "$DMG"; then
    echo "  ✓ notarized + stapled"
  fi
fi

echo
echo "✓ Built $DMG"
echo "  Open it and drag e.app into Applications."
if [[ "$IDENTITY" == "-" ]]; then
  echo "  (ad-hoc signed — see docs/installation.md for Developer ID + notarization)"
fi
exit 0
