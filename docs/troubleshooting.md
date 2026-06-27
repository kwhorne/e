# Troubleshooting

## No completion / diagnostics / go-to-definition

These features require a language server on your `PATH`.

- Check it's installed: e.g. `which intelephense`, `which rust-analyzer`.
- See [Installation → Language servers](installation.md#language-servers).
- `rustup`'s `rust-analyzer` shim only works after
  `rustup component add rust-analyzer`.

## The window doesn't come to the front (macOS)

Launching the bare binary from a terminal may leave the window behind. Use the
app bundle instead:

```sh
./scripts/run.sh path/to/project
```

## "Open another project" did nothing

`⌘O` opens the chosen folder in a **new** window. Check whether a new window
appeared (it may be behind the current one). The current project is left intact.

## Git features show nothing

The Source Control panel and blame require the workspace to be inside a git
repository, and the `git` command-line tool to be installed and on your `PATH`.

## The gutter still shows changes right after a commit

The change gutter for an open file refreshes when the file is reopened or saved.
The Source Control panel itself updates immediately.

## Auto-update can't install

- Ensure you're running a released binary (not a `cargo run` dev build).
- Check your network connection and that
  [GitHub Releases](https://github.com/kwhorne/e/releases) is reachable.
- Use **Retry** in the notice, or download the latest release manually.

## High CPU or an unresponsive window

This shouldn't happen — if it does, please
[open an issue](https://github.com/kwhorne/e/issues) with your OS version and
what you were doing. Include any output from running `e` in a terminal.

## Reset configuration

Delete `~/.config/e/config.json` to restore defaults. Session state lives in
`~/.config/e/sessions/`.
