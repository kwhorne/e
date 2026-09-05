# Troubleshooting

## No completion / diagnostics / go-to-definition

These features require a language server on your `PATH`.

- Check it's installed: e.g. `which intelephense`, `which rust-analyzer`.
- See [Installation → Language servers](installation.md#language-servers).
- `rustup`'s `rust-analyzer` shim only works after
  `rustup component add rust-analyzer`.

## Started from the Dock, everything is "not installed"

The status bar says *intelephense ↓ click to install* though it is installed,
`⌘⇧A` says *No Artisan commands*, tasks can't find `php` — but the same app
started from a terminal finds them all. An app launched from the Dock, Finder
or `open` inherits launchd's PATH (`/usr/bin:/bin:/usr/sbin:/sbin`), not your
shell's, and Grove's `~/.grove/bin`, `~/.npm-global/bin` and Composer's
`vendor/bin` aren't in it.

`e` asks your login shell for its PATH at startup (`$SHELL -ilc`, so both the
profile and the rc file run) and adopts it, so this should not happen from
0.9.20 on. If a tool is still missing, check that your shell finds it
(`which php`) and that the PATH line lives in a file the login shell reads
(`~/.zprofile` or `~/.zshrc` for zsh). `E_NO_SHELL_PATH=1` skips the lookup;
a shell that takes more than four seconds to start is skipped too.

## A shortcut does nothing

Shortcuts match the key you press, not what it would type: `⌘⇧,` is the comma
key even though ⇧, is `;` on a Norwegian keyboard and `<` on a US one, and
`⌘⌥E` is the E key even though ⌥E is a dead key (accent composition is
switched off while ⌘ is held). A chord can still be taken before it reaches
any app — on one Mac `⌘⌥N` never arrived while `⌘⇧N` did — which is why New
Eloquent Model is `⌘⇧N`. If a chord does nothing,
start `e` from a terminal with `E_DEBUG_KEYS=1 e --foreground .` and press it:
every chord is logged with the key macOS reported, the physical key and the
command it resolved to (or `None`). Bindings are listed in
[keyboard-shortcuts.md](keyboard-shortcuts.md) and overridable in
`config.json`.

## `e .` used to hang the terminal

Since 0.9.17 the command hands the shell back immediately and the editor
survives the terminal closing. Server and project messages go to
`~/.config/e/e.log`; `e --foreground .` keeps them in the terminal.

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

## "Couldn't check for updates … status: 403"

That was GitHub's API rate limit (60 calls an hour per address without a
login). Since 0.9.18 the check doesn't use the API, so this no longer happens;
if you see it on an older build, wait an hour or update by hand from the
releases page.

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
