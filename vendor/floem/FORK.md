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
- `cargo clippy --workspace` does **not** touch this fork (it's excluded from our
  workspace), so upstream's own lint warnings — e.g. the pre-existing
  `unused_assignments` in `src/animate.rs` — don't fail our CI. Keep our own
  changes clean regardless.
- `cargo fmt --all` **does** reach files here, despite the exclusion, and CI runs
  it with `--check`. Format anything you add.
- CI runs `cargo test --manifest-path vendor/floem/editor-core/Cargo.toml` as its
  own step, because `--workspace` cannot see this tree. Tests covering our own
  changes to the fork belong there and will be run.
- Keep changes small and well-commented so a future re-sync with upstream is
  tractable. When you change a view, note *why* (which editor feature needs it).

## Re-syncing with upstream (later)

There is no live git remote here. To pull newer upstream changes, clone
`lapce/floem` at the target revision, diff it against this tree, and port our
local changes forward. Record any local patches in this file as they land:

### Local changes on top of upstream

- **`editor-core/src/buffer/mod.rs`: `Buffer::edit` can no longer abort the
  process.** Two shapes of input reached the CRDT engine as a malformed delta
  and tripped an assertion — a selection whose offsets exceed the buffer
  (`lapce-xi-rope` `multiset.rs`: *"self must cover all 0-regions of other"*),
  and two selections in one `edit` call that overlap each other (`delta.rs:594`).
  Both fire inside a callback that cannot unwind, so Rust aborts instead of
  panicking and unsaved work is lost. This was not theoretical: it is in
  `~/.config/e/crash.log`, reached from `receive_char` — ordinary typing.

  Regions are now clamped to the buffer length, and a region overlapping or
  repeating one already accepted is dropped. A single `Selection` merges its own
  overlaps, but `edit` takes several and nothing reconciled them against each
  other. The interval sort also became a plain total order by `(start, end)`;
  it agrees with the old comparator for the disjoint regions that are normal,
  and unlike it is well-defined when they are not.

  Covered by `editor-core/tests/edit_robustness.rs`, including a 2000-case
  generated sweep of degenerate selections that must not abort.

- **`src/views/rich_text.rs`: text selection.** Added an opt-in
  `RichText::selectable()` (+ `selection_color()`) that ports the pointer /
  copy / highlight machinery from `Label` into `RichText`, so styled rich text
  (bold, inline code, colors) can now be drag-selected and copied with
  `Cmd/Ctrl+C`. Upstream `rich_text` had no selection at all — this removes the
  “selectable *or* styled, pick one” limitation we were working around with
  single-style labels.
- **`f32` literal suffixes (`inspector/view.rs`, `profiler.rs`, `views/resizable.rs`).**
  Annotated 14 `flex_grow(1.0)` / `flex_grow(1.)` call sites as `1.0_f32`. Rust is
  phasing out the `f64 → f32` inference fallback (rust-lang/rust#154024) and it
  **will become a hard error**, so this is a forward-compatibility fix rather than
  a style change. Note `cargo fix` gets this wrong for `1.` (it produces `1._f32`,
  which parses as field access) — these were applied by hand.
- **`src/views/rich_text.rs`: click offsets.** Added `RichText::on_click_offset(cb)`
  which fires on a plain click (not a drag-selection) with the byte offset of the
  hit character. Used to make file paths in terminal/agent output click-to-open.
