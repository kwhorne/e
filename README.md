# e

A lightning-fast code editor written in Rust, inspired by [Lapce](https://github.com/lapce/lapce).
UI built on [Floem](https://github.com/lapce/floem).

## Status

Early development. Built incrementally:   jhdjshj

- [x] **Milepæl 0** — Skjelett: open a file, edit it, save with Cmd/Ctrl+S, status bar
- [x] **Milepæl A** — Workspace: file tree, tabs, command palette (⌘P), multi-buffer
- [x] **Milepæl 3** — Syntaks: tree-sitter (Rust, Python, JS/TS, Go, C, JSON, PHP, HTML, CSS, Blade, Vue, Svelte)
- [x] **Milepæl 4** — LSP: PHP via Intelephense (diagnostics, completion, hover)
- [x] **Milepæl 5** — Laravel-lag: `route()`/`view()`/`config()`/`env()`-completion
- [x] **Milepæl 6** — Go-to-definition (F12), auto-scroll til mål
- [x] **Milepæl 7C** — Go-to-references (Shift+F12) + symbol-søk (⌘T)
- [x] **Milepæl 7A** — Inline bølgelinjer (squiggles) under diagnostics i editoren
- [x] **Milepæl 7B** — Integrert terminal (PTY + VT100, Ctrl+`)
- [x] **Milepæl 8** — Format-on-save (PHP) + git-gutter (endrede linjer)
- [x] **Milepæl 9** — Document outline-panel (LSP documentSymbol) i sidebar
- [x] **Milepæl 10** — Signature help (popup med aktiv-parameter-highlight)
- [x] **Milepæl 11** — Find-in-file (⌘F), terminal-farger (SGR), split-view (⌘\\)
- [x] **Milepæl 12** — Workspace-wide search / grep (⌘⇧F)
- [x] **Milepæl 13** — Session-persistens (gjenåpner filer/faner/split per workspace)
- [x] **Milepæl 14** — Breadcrumbs (sti + symbol ved cursor)
- [x] **Milepæl 15** — Auto-save (lagrer skitne buffere etter 1,5 s inaktivitet)
- [x] **Milepæl 16** — Tema-system (lys/mørk, F8) — reaktiv palett + editor-tema
- [x] **Milepæl 17** — Terminal-resize til panelstørrelse (on_resize → PTY/grid)
- [x] **Milepæl 18** — Lokal rename-i-fil (F2, whole-word, virker uten LSP premium)
- [x] **Milepæl 19** — Markdown-preview (⌘⇧M reading-mode for .md)
- [x] **Milepæl 20** — Tema-persistens · command palette (⌘⇧P) · git diff-view · multi-cursor (⌘D)
- [x] **Milepæl 21** — Multi-språk LSP (PHP/intelephense, C·C++/clangd, Rust, TS, Go, Python)
- [x] **Milepæl 22** — Bracket-matching (highlight av matchende parentes ved cursor)
- [x] **Milepæl 23** — Snippets (innebygde kode-maler per språk, $0-cursor, i completion)
- [x] **Milepæl 24** — Bruker-settings (config.json: font/tab/format-on-save/autosave/indent-guides)
- [x] **Milepæl 25** — Statuslinje: cursor-posisjon (Ln/Col) + seleksjonslengde
- [x] **Milepæl 26** — Trailing-whitespace-trim + final newline ved lagring
- [x] **Milepæl 27** — Workspace-wide problems-panel (alle filer, gruppert, klikkbart)
- [x] **App-ikon** — orange "e" + terminal-cursor på mørk bakgrunn (SVG → .icns) + macOS-bundle-script

## Workspace layout

```
e/
├── e-lsp/    # JSON-RPC LSP client over stdio (Intelephense)       ~ lapce-proxy
├── e-term/   # PTY + VT100 terminal model                          ~ lapce-proxy
├── e-core/   # text IO, language detection, tree-sitter syntax    ~ lapce-core
│   ├── buffer.rs    # file load/save
│   ├── language.rs  # extension -> Language
│   └── syntax.rs    # tree-sitter -> per-line highlight spans
└── e-app/    # Floem UI + the `e` binary                          ~ lapce-app
    ├── state.rs        # reactive AppState, buffers, LSP, terminal
    ├── file_tree.rs    # left explorer
    ├── tabs.rs         # tab strip
    ├── editor_area.rs  # multi-buffer editor
    ├── styling.rs      # syntax highlight + diagnostic squiggles
    ├── palette.rs      # ⌘P fuzzy finder
    ├── completion.rs   # completion + hover popups
    ├── laravel.rs      # route/view/config/env completion
    ├── picker.rs       # references + symbol search overlay
    ├── problems.rs     # clickable LSP diagnostics panel
    ├── terminal_view.rs# integrated terminal panel
    └── status.rs       # status bar (+ error/warning counts)
```

## PHP / Laravel

Opening a `.php` file auto-starts [Intelephense](https://intelephense.com)
(`intelephense --stdio`). You get **diagnostics**, **completion** (auto as you
type, or ⌘Space) and **hover** (F1). Install Intelephense once:

```sh
npm install -g intelephense
```

## Other language servers

`e` auto-detects and launches the right server per language (if installed):
clangd (C/C++), rust-analyzer (Rust), typescript-language-server, gopls,
pyright. Servers that fail to start are skipped silently.

### Laravel awareness

If the workspace contains an `artisan` file, `e` scrapes the project in the
background (inspired by the official Laravel VS Code extension) and offers
context-aware completion inside helper strings:

| Inside | Completes |
|--------|-----------|
| `route('…')` | route names (`php artisan route:list --json`) |
| `view('…')` | Blade views as dotted names (`resources/views/**`) |
| `config('…')` | config files + first-level keys (`config/*.php`) |
| `env('…')` | keys from `.env` |

Works in both `.php` and `.blade.php` files.

### Keybindings

| Key | Action |
|-----|--------|
| ⌘P / Ctrl+P | Find file |
| ⌘S / Ctrl+S | Save |
| ⌘Space | Trigger completion |
| ↑/↓ + Enter/Tab | Navigate / accept completion |
| F1 | Hover info |
| F12 | Go to definition |
| Shift+F12 | Find references |
| ⌘T | Toggle terminal |
| ⌘⇧O | Workspace symbol search |
| ⌘⇧F | Search across files |
| ⌘F | Find in file |
| ⌘\\ | Toggle split view |
| Ctrl+` | Toggle terminal (alt.) |
| F8 | Toggle light/dark theme |
| F2 | Rename in file |
| ⌘⇧M | Markdown preview (.md) |
| ⌘⇧P | Command palette |
| ⌘D | Add cursor at next occurrence |
| Esc | Dismiss popups |

## Build & run

```sh
cargo run -- path/to/file.rs
```

The first build is heavy — Floem pulls in wgpu and a large dependency tree.

## License

Apache-2.0
