# Updating

`e` has a built-in auto-updater backed by GitHub Releases.

## How it works

- On startup, `e` quietly checks GitHub for a newer release.
- When one is available, a notice appears in the **bottom-right** corner with the
  release version and a **What's new** toggle that expands the changelog.
- Click **Update now** to download the build for your platform and replace the
  running binary in place.
- After it installs, click **Restart now** to relaunch into the new version.

## Manual check

Run **Check for Updates** from the command palette (`⌘⇧P`). If you're already on
the latest version, you'll see a confirmation.

## Notes

- The updater downloads the correct asset for your platform
  (`e-<target>.tar.gz`) and swaps the binary atomically.
- If a download fails, the notice offers **Retry**.
- Updates are opt-in: nothing is installed until you click **Update now**.

## For maintainers

Releases are produced with:

```sh
./scripts/release.sh X.Y.Z
```

This bumps the version, moves the `Unreleased` section of `CHANGELOG.md` under a
new dated heading, commits, tags `vX.Y.Z`, and pushes. A GitHub Actions workflow
then builds and attaches the per-platform binaries to the release, which the
auto-updater consumes.
