# Configuration

All settings live in a single global JSON file:

```
~/.config/e/config.json
```

Press **`⌘,`** for a graphical settings page covering the common options. It's a
two-pane dialog: pick a category (Appearance, Editor, On Save, Panels, Laravel,
Agents) in the sidebar, or type in the search box to filter settings across every
category. Changes are saved automatically; the **Open config.json** link in the
footer jumps to the raw file (with advanced sections like `agents`, `snippets`
and `keybindings`), which is created on first use.

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
  "inlay_hints": true,     // show LSP inlay hints (types, parameter names)
  "sticky_scroll": true,   // pin enclosing scope lines at the top while scrolling
  "ai_completion": false,  // inline AI "ghost text" via a local Ollama code model
  "native_agent": true,    // native chat panel for Elyra (vs. terminal); see Agents
  "xdebug": false,         // enable Xdebug step-debugging via Grove (grove debug on)
  "sidebar_side": "left",  // explorer/Git panel side: "left" or "right"
  "agent_side": "right",   // agent panel side: "right" or "left"
  "keybindings": { "cmd+k": "delete-line" }, // override or add bindings
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
| `inlay_hints` | bool | `true` | Show LSP inlay hints |
| `sticky_scroll` | bool | `true` | Pin enclosing scope at the top |
| `ai_completion` | bool | `false` | Inline AI completion (ghost text) via a local Ollama code model; `Tab` accepts. Model via `E_COMPLETION_MODEL` (default `qwen2.5-coder`). Requires a running Ollama. |
| `native_agent` | bool | `true` | Use the native chat panel for Elyra (streaming markdown, tool cards, composer) instead of the terminal panel. See [AI Agents](agents.md). |
| `xdebug` | bool | `false` | Toggling this runs `grove debug on`/`off` to load Xdebug for step-debugging (see [Debugging](debugging.md)) |
| `sidebar_side` | string | `"left"` | Side of the explorer/Git sidebar (`"left"` or `"right"`) |
| `agent_side` | string | `"right"` | Side of the agent panel (`"right"` or `"left"`) |
| `agents` | object | built-ins | AI agent configuration |

> **Panel layout** (`sidebar_side`, `agent_side`) is read at startup — restart
> `e` after changing it. By default the explorer/Git sidebar is on the left and
> the agent panel on the right; set them to swap sides.

## EditorConfig

If a project has an [`.editorconfig`](https://editorconfig.org), `e` honours it
per file, layered over the global settings above:

- `indent_size` / `tab_width` → the buffer's tab width.
- `trim_trailing_whitespace` → overrides `trim_on_save` for matching files.
- `insert_final_newline` → ensures a single trailing newline on save.

The usual resolution rules apply: files are read from the file's directory
upward, stopping at `root = true`, and a nearer file (or a later matching
section) wins. Globs support `*`, `**`, `?` and `{a,b}`.

## Keybindings

Every action is a named command; rebind any of them in the `keybindings` section.
See [Keybindings](keybindings.md) for the syntax and the full command list.

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
