#!/usr/bin/env bash
#
# Cut a new release of `e`.
#
#   ./scripts/release.sh 0.2.0
#
# What it does:
#   1. Validates the version and a clean working tree on `main`.
#   2. Bumps the workspace version in Cargo.toml (and refreshes Cargo.lock).
#   3. Moves the CHANGELOG "Unreleased" section under a new dated version
#      heading and updates the comparison links.
#   4. Commits and pushes main, then waits for CI on that commit.
#   5. Only when CI passes: tags `vX.Y.Z` and pushes the tag.
#   6. Builds the universal DMG on this Mac, signed with Knut Horne's
#      Developer ID and notarized with the "Knut Horne" keychain profile (as
#      eterm's releases are), and attaches it to the release.
#
# The GitHub release workflow (.github/workflows/release.yml) then builds and
# attaches the platform archives automatically. Tagging only a commit CI has
# passed is the point of step 4: the release workflow runs no tests of its own.
# Without `gh`, or with E_RELEASE_NO_CI_WAIT=1, the tag goes out unchecked.

set -euo pipefail
cd "$(dirname "$0")/.."

NEW="${1:-}"
if [[ ! "$NEW" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "usage: $0 <major.minor.patch>   (e.g. 0.2.0)" >&2
  exit 1
fi

# --- sanity checks ---------------------------------------------------------
# The DMG is signed and notarized here, so check that this Mac can before
# anything is committed or pushed (E_RELEASE_NO_DMG=1 skips the DMG).
if [[ "${E_RELEASE_NO_DMG:-}" != "1" ]]; then
  if [[ "$(uname)" != "Darwin" ]]; then
    echo "error: the DMG is built on macOS; run this on a Mac or set E_RELEASE_NO_DMG=1" >&2
    exit 1
  fi
  if ! xcrun notarytool history --keychain-profile "${NOTARY_PROFILE:-Knut Horne}" >/dev/null 2>&1; then
    echo "error: no notarization credentials under '${NOTARY_PROFILE:-Knut Horne}'" >&2
    echo "       xcrun notarytool store-credentials \"Knut Horne\" --apple-id … --team-id 52AT44QFCM --password …" >&2
    exit 1
  fi
fi

branch="$(git rev-parse --abbrev-ref HEAD)"
if [[ "$branch" != "main" ]]; then
  echo "error: must be on 'main' (currently on '$branch')" >&2
  exit 1
fi
if [[ -n "$(git status --porcelain)" ]]; then
  echo "error: working tree is not clean — commit or stash first" >&2
  exit 1
fi

PREV="$(grep -E '^[[:space:]]*version[[:space:]]*=[[:space:]]*"' Cargo.toml | head -1 | sed -E 's/.*"([0-9.]+)".*/\1/')"
if [[ "$NEW" == "$PREV" ]]; then
  echo "error: version is already $NEW" >&2
  exit 1
fi
if git rev-parse "v$NEW" >/dev/null 2>&1; then
  echo "error: tag v$NEW already exists" >&2
  exit 1
fi

DATE="$(date +%Y-%m-%d)"
REPO="https://github.com/kwhorne/e"
echo "==> releasing $PREV -> $NEW ($DATE)"

# --- 1. bump Cargo.toml ----------------------------------------------------
perl -0pi -e 's/^(version\s*=\s*)"'"$PREV"'"/$1"'"$NEW"'"/m' Cargo.toml
# Refresh Cargo.lock entries for the workspace crates.
cargo update --workspace --quiet 2>/dev/null || cargo check --quiet

# --- 2. rewrite CHANGELOG --------------------------------------------------
# Turn the empty "Unreleased" heading into a fresh one plus the new version
# heading; everything previously under Unreleased becomes the release body.
perl -0pi -e 's/## \[Unreleased\]\n/## [Unreleased]\n\n## ['"$NEW"'] - '"$DATE"'\n/' CHANGELOG.md

# Update link references at the bottom.
perl -0pi -e 's{\[Unreleased\]: '"$REPO"'/compare/v'"$PREV"'\.\.\.HEAD}{[Unreleased]: '"$REPO"'/compare/v'"$NEW"'...HEAD\n['"$NEW"']: '"$REPO"'/compare/v'"$PREV"'...v'"$NEW"'}' CHANGELOG.md

# --- 3. commit and push main ----------------------------------------------
git add Cargo.toml Cargo.lock CHANGELOG.md
git commit -q -m "Release v$NEW"
echo "==> pushing main"
git push origin main

# --- 4. wait for CI on the release commit ----------------------------------
SHA="$(git rev-parse HEAD)"
if [[ "${E_RELEASE_NO_CI_WAIT:-}" == "1" ]]; then
  echo "==> E_RELEASE_NO_CI_WAIT=1: not waiting for CI"
elif ! command -v gh >/dev/null 2>&1; then
  echo "warning: gh is not installed; tagging without waiting for CI" >&2
else
  echo "==> waiting for CI on ${SHA:0:7}"
  RUN=""
  for _ in $(seq 1 36); do
    RUN="$(gh run list --workflow ci.yml --commit "$SHA" --json databaseId -q '.[0].databaseId' 2>/dev/null || true)"
    [[ -n "$RUN" ]] && break
    sleep 5
  done
  if [[ -z "$RUN" ]]; then
    echo "error: no CI run started for ${SHA:0:7} within 3 minutes; not tagging." >&2
    echo "       Once CI is green: git tag -a v$NEW -m 'e $NEW' && git push origin v$NEW" >&2
    exit 1
  fi
  if ! gh run watch "$RUN" --exit-status --interval 20; then
    echo "error: CI failed on the release commit; v$NEW was not tagged." >&2
    echo "       Fix it on main, then tag the fixed commit:" >&2
    echo "       git tag -a v$NEW -m 'e $NEW' && git push origin v$NEW" >&2
    exit 1
  fi
fi

# --- 5. tag and push the tag -----------------------------------------------
git tag -a "v$NEW" -m "e $NEW"
echo "==> pushing v$NEW"
git push origin "v$NEW"

# --- 6. the DMG, signed and notarized on this Mac ------------------------------
# The workflow builds the archives in CI; the installer is made here, where the
# Developer ID and the notarization credentials are, as eterm's is. It fails
# rather than attach a DMG Apple hasn't notarized.
if [[ "${E_RELEASE_NO_DMG:-}" == "1" ]]; then
  echo "==> E_RELEASE_NO_DMG=1: no DMG"
else
  echo "==> building, signing and notarizing the universal DMG"
  E_REQUIRE_NOTARIZATION=1 ./scripts/bundle-dmg.sh --universal
  DMG="e-$NEW-universal.dmg"
  (cd dist && shasum -a 256 "$DMG" > "$DMG.sha256")
  # The workflow makes the release when its first archive is attached.
  for _ in $(seq 1 90); do
    gh release view "v$NEW" >/dev/null 2>&1 && break
    sleep 10
  done
  gh release upload "v$NEW" "dist/$DMG" "dist/$DMG.sha256" --clobber
fi

echo
echo "✓ Released v$NEW."
echo "  → GitHub Actions is building the archives:"
echo "    $REPO/actions"
echo "  → Release page: $REPO/releases/tag/v$NEW"
