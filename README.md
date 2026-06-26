# e

A lightning-fast code editor written in Rust, inspired by [Lapce](https://github.com/lapce/lapce).
UI built on [Floem](https://github.com/lapce/floem).

## Status

Early development. Built incrementally:

- [x] **Milepæl 0** — Skjelett: open a file, edit it, save with Cmd/Ctrl+S, status bar
- [ ] **Milepæl 1** — Editor-kjerne: cursor, selection, undo/redo polish, save-as
- [ ] **Milepæl 2** — Workspace: file tree, tabs, command palette (Ctrl+P)
- [ ] **Milepæl 3** — Syntaks: tree-sitter highlighting
- [ ] **Milepæl 4** — LSP: completion, diagnostics
- [ ] **Milepæl 5** — Terminal / plugins

## Workspace layout

```
e/
├── e-core/   # text, language detection, syntax (GUI-agnostic)   ~ lapce-core
└── e-app/    # Floem UI + the `e` binary                          ~ lapce-app
```

## Build & run

```sh
cargo run -- path/to/file.rs
```

The first build is heavy — Floem pulls in wgpu and a large dependency tree.

## License

Apache-2.0
