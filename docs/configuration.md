# Configuration

All settings live in a single global JSON file:

```
~/.config/e/config.json
```

Open it any time with **`⌘,`** (it is created on first use).

## Settings reference

```jsonc
{
  "dark": true,            // dark theme (false = light)
  "font_size": 14,         // editor font size (8–40)
  "tab_width": 4,          // spaces per indent level (1–16)
  "format_on_save": true,  // format via the language server on save
  "trim_on_save": true,    // trim trailing whitespace + ensure final newline
  "autosave": true,        // save dirty buffers after a short idle period
  "indent_guides": true,   // show indentation guide lines
  "auto_close": true,      // auto-close brackets and quotes
  "agents": { /* see below */ }
}
```

| Key | Type | Default | Description |
| --- | ---- | ------- | ----------- |
| `dark` | bool | `true` | Dark or light theme |
| `font_size` | int | `14` | Editor font size (clamped 8–40) |
| `tab_width` | int | `4` | Indent width (clamped 1–16) |
| `format_on_save` | bool | `true` | Format the document on save |
| `trim_on_save` | bool | `true` | Trim trailing whitespace on save |
| `autosave` | bool | `true` | Idle auto-save |
| `indent_guides` | bool | `true` | Indentation guides |
| `auto_close` | bool | `true` | Auto-close brackets/quotes |
| `agents` | object | built-ins | AI agent configuration |

## Themes

`e` ships with a light and a dark theme. Toggle between them with **`F8`**; the
choice is saved to `config.json`. The theme is fully reactive — the whole UI and
editor update instantly.

## Zoom & word wrap

- **`⌘=` / `⌘-`** change the editor font size for the session; **`⌘0`** resets to
  your configured `font_size`.
- **`⌥Z`** toggles soft word wrap.

## Agents

See [AI Agents](agents.md) for the `agents` section (default agent and the list
of configurable agents).

## Where things are stored

| Path | Contents |
| ---- | -------- |
| `~/.config/e/config.json` | Global settings |
| `~/.config/e/sessions/` | Per-workspace session state (open files, tabs, split) |

Sessions are restored automatically when you reopen a workspace.
