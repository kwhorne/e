#!/usr/bin/env bash
#
# Shared code-signing / notarization helpers for `e`, sourced by the bundle
# scripts. Secrets never live in the repo:
#
#   • Locally, notarization uses a Keychain profile (default: "e-notary"),
#     created once with:
#       xcrun notarytool store-credentials e-notary \
#         --apple-id <you@example.com> --team-id <TEAMID> --password <app-pw>
#
#   • In CI, set NOTARY_APPLE_ID / NOTARY_PASSWORD / NOTARY_TEAM_ID (and import
#     a Developer ID cert), and these take precedence over the profile.
#
# The signing identity is auto-detected from the keychain unless
# CODESIGN_IDENTITY is set.

# Print the Developer ID Application identity, or empty if none is available.
detect_identity() {
  if [[ -n "${CODESIGN_IDENTITY:-}" ]]; then
    echo "$CODESIGN_IDENTITY"
    return
  fi
  security find-identity -v -p codesigning 2>/dev/null \
    | awk -F'"' '/Developer ID Application/{print $2; exit}'
}

# sign_app <app-path> <identity>
sign_app() {
  local app="$1" id="$2"
  if [[ -z "$id" || "$id" == "-" ]]; then
    echo "==> ad-hoc signing $app (no Developer ID)"
    codesign --force --deep --sign - "$app" 2>/dev/null || true
    return
  fi
  echo "==> signing $app with: $id"
  # Inside-out: nested executable first, then the bundle. Hardened runtime +
  # secure timestamp are required for notarization.
  codesign --force --options runtime --timestamp --sign "$id" "$app/Contents/MacOS/e"
  codesign --force --options runtime --timestamp --sign "$id" "$app"
  codesign --verify --strict --verbose=2 "$app"
}

# sign_dmg <dmg-path> <identity>
sign_dmg() {
  local dmg="$1" id="$2"
  [[ -z "$id" || "$id" == "-" ]] && return 0
  echo "==> signing $dmg"
  codesign --force --timestamp --sign "$id" "$dmg"
}

# notarize_staple <path>  (a .dmg, .zip or .pkg)
# Returns non-zero (and skips) if no credentials are configured.
notarize_staple() {
  local target="$1"
  local args=()
  if [[ -n "${NOTARY_APPLE_ID:-}" && -n "${NOTARY_PASSWORD:-}" && -n "${NOTARY_TEAM_ID:-}" ]]; then
    args=(--apple-id "$NOTARY_APPLE_ID" --password "$NOTARY_PASSWORD" --team-id "$NOTARY_TEAM_ID")
  elif [[ -n "${NOTARY_PROFILE:-}" ]]; then
    args=(--keychain-profile "$NOTARY_PROFILE")
  else
    echo "==> notarization skipped (no NOTARY_PROFILE or NOTARY_APPLE_ID/PASSWORD/TEAM_ID)"
    return 1
  fi
  echo "==> notarizing $target (this can take a few minutes)…"
  xcrun notarytool submit "$target" "${args[@]}" --wait || return 1
  echo "==> stapling $target"
  xcrun stapler staple "$target"
}
