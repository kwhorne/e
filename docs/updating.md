# Updating

`e` has a built-in auto-updater backed by GitHub Releases.

## How it works

- On startup (at most every six hours), `e` quietly checks for a newer release.
  It reads the version from where `github.com/kwhorne/e/releases/latest`
  redirects to and the notes from that tag's `CHANGELOG.md` — not from the
  GitHub API, whose 60 unauthenticated calls an hour per address a day of
  restarts (or an office behind one IP) uses up. The API is only a fallback.
- When one is available, a notice appears in the **bottom-right** corner with the
  release version and a **What's new** toggle that expands the changelog.
- Click **Update now** to download the build for your platform and replace the
  running binary in place.
- After it installs, click **Restart now** to relaunch into the new version.

## Manual check

Run **Check for Updates** from the command palette (`⌘⇧P`). If you're already on
the latest version, you'll see a confirmation.

## Notes

- On macOS, an `e` installed as `e.app` is updated as a whole app: the updater
  downloads the release's signed, notarized `e-<version>-universal.app.zip`,
  checks it against the `.sha256` published beside it, and only puts it in
  place once `codesign` confirms every file is sealed by e's Developer ID
  (team `52AT44QFCM`) and it is the version offered. The old app is moved
  aside and the new one renamed into its place. Replacing just the binary
  inside the app would break its signature, and with it Gatekeeper's trust,
  Keychain access and the permissions you granted `e`.
- The updater needs to write to the folder the app is in (usually
  `/Applications`). If it can't, it says so; download the DMG instead.
- Elsewhere (Linux, or a bare `e` binary) it downloads the archive for your
  platform (`e-<target>.tar.gz`), verifies it against its `.sha256`, and swaps
  the binary atomically.
- If a download fails, the notice offers **Retry**.
- Updates are opt-in: nothing is installed until you click **Update now**.

## For maintainers

Releases are produced with:

```sh
./scripts/release.sh X.Y.Z
```

This bumps the version, moves the `Unreleased` section of `CHANGELOG.md` under a
new dated heading, commits, tags `vX.Y.Z`, and pushes. A GitHub Actions workflow
then builds and attaches the per-platform binaries to the release. The script
itself builds, signs and notarizes the universal DMG on the Mac and attaches it
along with `e-X.Y.Z-universal.app.zip`, the app the macOS updater installs. A
release without that zip can't be installed by the updater on macOS.
