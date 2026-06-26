# e

A lightning-fast code editor written in Rust, inspired by [Lapce](https://github.com/lapce/lapce).
UI built on [Floem](https://github.com/lapce/floem).

## Status

Early development. Built incrementally:

- [x] **Milepæl 0** — Skjelett: open a file, edit it, save with Cmd/Ctrl+S, status bar
- [x] **Milepæl A** — Workspace: file tree, tabs, command palette (⌘P), multi-buffer
- [x] **Milepæl 3** — Syntaks: tree-sitter (Rust, Python, JS/TS, Go, C, JSON, PHP, HTML, CSS, Blade, Vue, Svelte)
- [x] **Milepæl 4** — LSP: PHP via Intelephense (diagnostics, completion, hover)
- [ ] **Milepæl 5** — Laravel-lag (artisan: routes/views/config-completion), terminal

## Workspace layout

```
e/
├── e-lsp/    # JSON-RPC LSP client over stdio (Intelephense)       ~ lapce-proxy
├── e-core/   # text IO, language detection, tree-sitter syntax    ~ lapce-core
│   ├── buffer.rs    # file load/save
│   ├── language.rs  # extension -> Language
│   └── syntax.rs    # tree-sitter -> per-line highlight spans
└── e-app/    # Floem UI + the `e` binary                          ~ lapce-app
    ├── state.rs       # reactive AppState, buffers, open/close/save
    ├── file_tree.rs   # left explorer
    ├── tabs.rs        # tab strip
    ├── editor_area.rs # multi-buffer editor
    ├── styling.rs     # syntax-highlight Styling (monospace)
    ├── palette.rs     # ⌘P fuzzy finder
    ├── problems.rs    # LSP diagnostics panel
    └── status.rs      # status bar (+ error/warning counts)
```

## PHP / Laravel

Opening a `.php` file auto-starts [Intelephense](https://intelephense.com)
(`intelephense --stdio`). You get **diagnostics**, **completion** (auto as you
type, or ⌘Space) and **hover** (F1). Install Intelephense once:

```sh
npm install -g intelephense
```

### Keybindings

| Key | Action |
|-----|--------|
| ⌘P / Ctrl+P | Find file |
| ⌘S / Ctrl+S | Save |
| ⌘Space | Trigger completion |
| ↑/↓ + Enter/Tab | Navigate / accept completion |
| F1 | Hover info |
| Esc | Dismiss popups |

## Build & run

```sh
cargo run -- path/to/file.rs
```

The first build is heavy — Floem pulls in wgpu and a large dependency tree.

## License

Apache-2.0
