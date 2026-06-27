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
#   4. Commits, tags `vX.Y.Z`, and pushes main + the tag.
#
# The GitHub release workflow (.github/workflows/release.yml) then builds and
# attaches the platform binaries automatically.

set -euo pipefail
cd "$(dirname "$0")/.."

NEW="${1:-}"
if [[ ! "$NEW" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "usage: $0 <major.minor.patch>   (e.g. 0.2.0)" >&2
  exit 1
fi

# --- sanity checks ---------------------------------------------------------
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

# --- 3. commit, tag, push --------------------------------------------------
git add Cargo.toml Cargo.lock CHANGELOG.md
git commit -q -m "Release v$NEW"
git tag -a "v$NEW" -m "e $NEW"

echo "==> pushing main + v$NEW"
git push origin main
git push origin "v$NEW"

echo
echo "✓ Released v$NEW."
echo "  → GitHub Actions is building the release binaries now:"
echo "    $REPO/actions"
echo "  → Release page: $REPO/releases/tag/v$NEW"
