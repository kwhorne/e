# Keybindings

Every action in `e` is a named **command**. Keys are bound to commands in a
table, and you can override or add bindings in your
[`config.json`](configuration.md) (`⌘,`).

## Customizing

Add a `keybindings` section that maps a key string to a command id:

```jsonc
{
  "keybindings": {
    "cmd+k": "delete-line",       // rebind ⌘K
    "ctrl+shift+t": "run-test",   // add a new binding
    "cmd+/": "none"               // remove a default binding
  }
}
```

- A user binding **overrides** the default for that key.
- Use `"none"` (or `""`) as the command to **remove** a default binding.
- Restart `e` after editing.

## Key string syntax

Modifiers, then the key, joined with `+`:

- Modifiers: `cmd` (⌘), `ctrl`, `alt` (⌥), `shift`
- Keys: a letter/symbol (`p`, `/`, `=`, `-`, `[`), `space`, `escape`, `enter`,
  `tab`, `backspace`, arrows (`up`, `down`, `left`, `right`), and `f1`–`f12`

Examples: `cmd+shift+p`, `ctrl+g`, `alt+up`, `cmd+/`, `f12`, `shift+f12`.

## Command ids

| Command id | Default | Action |
| ---------- | ------- | ------ |
| `goto-file` | `cmd+p` | Find file |
| `command-palette` | `cmd+shift+p` | Command palette |
| `recent` | `cmd+e` | Recent files |
| `open-folder` | `cmd+o` | Open folder / project |
| `open-file` | — | Open file… |
| `symbols` | `cmd+shift+o` | Go to symbol |
| `search` | `cmd+shift+f` | Search in files |
| `find` | `cmd+f` | Find in file |
| `replace` | `cmd+alt+f` | Replace in file |
| `goto-line` | `ctrl+g` | Go to line |
| `nav-back` / `nav-forward` | `ctrl+-` / `ctrl+shift+-` | Navigate back / forward |
| `new-file` | `cmd+n` | New file |
| `save` / `save-as` | `cmd+s` / `cmd+shift+s` | Save / Save As |
| `format` | — | Format document |
| `rename` | `f2` | Rename symbol |
| `comment` | `cmd+/` | Toggle line comment |
| `move-line-up` / `move-line-down` | `alt+up` / `alt+down` | Move line |
| `duplicate-line` | `cmd+d` | Duplicate line |
| `delete-line` | `cmd+shift+k` | Delete line |
| `indent` / `outdent` | `cmd+]` / `cmd+[` | Indent / outdent |
| `select-next-occurrence` | `cmd+shift+d` | Add cursor at next match |
| `select-all-occurrences` | `cmd+shift+l` | Add cursor at all matches |
| `completion` | `cmd+space` | Trigger completion |
| `hover` | `f1` | Hover info |
| `definition` / `references` | `f12` / `shift+f12` | Go to definition / references |
| `run-task` / `run-test` | `cmd+shift+b` / — | Run task / tests |
| `source-control` | `cmd+2` | Toggle Source Control |
| `diff` | — | Git diff vs HEAD |
| `toggle-sidebar` | `cmd+1` | Toggle sidebar |
| `split` | `cmd+\` | Split editor |
| `toggle-terminal` | `cmd+t` / `ctrl+\`` | Toggle terminal |
| `new-terminal` / `split-terminal` | — | New / split terminal |
| `toggle-agent` / `restart-agent` | `cmd+l` / — | Agent panel / restart |
| `markdown` | `cmd+shift+m` | Markdown preview |
| `theme` | `f8` | Light / dark theme |
| `zoom-in` / `zoom-out` / `zoom-reset` | `cmd+=` / `cmd+-` / `cmd+0` | Zoom |
| `word-wrap` | `alt+z` | Toggle word wrap |
| `settings` | `cmd+,` | Open settings |
| `about` / `check-updates` / `install-cli` | — | App commands |
| `close` | `cmd+w` | Close tab / terminal / agent |
| `close-tab` | — | Close the active tab |
| `close-overlays` | `escape` | Dismiss open overlays |
