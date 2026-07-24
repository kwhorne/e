# Vendored Floem fork

This is a vendored fork of [Floem](https://github.com/lapce/floem), copied into
the `e` repository so we **own the UI toolkit** and are not limited by whatever
upstream does or doesn't support. When we hit a wall (non-selectable rich text,
no line-hiding for code folding, an input view that isn't flexible enough for the
agent composer, missing `application:openURLs:`), we now fix it *here* instead of
working around it.

## Provenance

- Upstream: `https://github.com/lapce/floem`
- Forked at revision: **`31fa8f444c37f4c314f47d88c23ffdbc25f2ab53`**
  (the same revision Lapce builds against).
- Copied from the Cargo git checkout; `.git/`, `examples/`, `docs/`,
  `.github/`, and `.devcontainer/` were dropped. `examples/*` was removed from
  the workspace `members` list in `Cargo.toml` accordingly.

## How it's wired

The root `Cargo.toml` points the workspace dependencies at this path instead of
the git source:

```toml
[workspace.dependencies.floem]
path     = "vendor/floem"
features = ["editor", "serde", "default-image-formats", "rfd-async-std"]

[workspace.dependencies.floem-editor-core]
path     = "vendor/floem/editor-core"
features = ["serde"]
```

`vendor/floem` is its own Cargo workspace, so it is listed under `exclude` in the
root `[workspace]` to keep the two from colliding. It still pulls its own
external deps (the `winit` / `muda` git forks) from their upstream sources — we
only own Floem's own crates here, not the whole dependency tree.

## Working in the fork

- Edit freely. `cargo build -p e-app` compiles this copy.
- `cargo clippy --workspace` and `cargo fmt --all` do **not** touch this fork
  (it's excluded from our workspace), so upstream's own lint warnings — e.g. the
  pre-existing `unused_assignments` in `src/animate.rs` — don't fail our CI. Keep
  our own changes clean regardless.
- Keep changes small and well-commented so a future re-sync with upstream is
  tractable. When you change a view, note *why* (which editor feature needs it).

## Re-syncing with upstream (later)

There is no live git remote here. To pull newer upstream changes, clone
`lapce/floem` at the target revision, diff it against this tree, and port our
local changes forward. Record any local patches in this file as they land:

### Local changes on top of upstream

- _(none yet — baseline import of `31fa8f44`)_
