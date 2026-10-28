# Contributing to e

Thanks for your interest in contributing! This document explains how to get set
up and the conventions we follow.

## Getting started

1. Install [Rust](https://rustup.rs) 1.87 or newer.
2. Fork and clone the repository.
3. Build and run:
   ```sh
   cargo build
   cargo run -- path/to/some/project
   ```
4. Run the test suite:
   ```sh
   cargo test --workspace
   ```

For language-server features you'll want the relevant servers on your `PATH`
(e.g. `intelephense`, `rust-analyzer`, `clangd`). Tests that depend on a server
skip themselves when it isn't installed.

## Project layout

| Crate     | Responsibility                                                |
| --------- | ------------------------------------------------------------- |
| `e-core`  | GUI-agnostic core: buffers, language detection, syntax, git   |
| `e-lsp`   | Language Server Protocol client                               |
| `e-term`  | PTY-backed terminal                                           |
| `e-app`   | The UI — editor, panels, palettes, theming, state             |
| `e`       | Binary entry point                                            |

## Before you open a pull request

- **Format** your code: `cargo fmt --all`
- **Lint**: `cargo clippy --workspace`
- **Test**: `cargo test --workspace`
- **UI flows** (macOS): `cargo test -p e-app --test ui_flows -- --ignored --test-threads=1`
  starts the real app on a small fixture project per test, drives it over its
  agent socket — keys, commands, the `⌘.` menu — and checks what `ui_state`
  says is on screen. Run it when a change touches the editor, the gutter or a
  flow end to end; add a flow in `e-app/tests/ui_flows.rs` for a new one.
- Keep commits focused and write clear, descriptive messages.
- If your change is user-facing, add an entry to [`CHANGELOG.md`](CHANGELOG.md)
  under the `Unreleased` section.
- Releases are cut with `scripts/release.sh <version>`: it bumps the version,
  dates the changelog, pushes `main`, waits for CI on that commit and only then
  tags it, which starts the release build.

## Reporting bugs & requesting features

Please use the GitHub issue templates. Include steps to reproduce, what you
expected, and what actually happened, along with your OS and `e` version.

## Code of Conduct

By participating you agree to abide by our
[Code of Conduct](CODE_OF_CONDUCT.md).

## License

By contributing, you agree that your contributions will be licensed under the
[MIT License](LICENSE).
