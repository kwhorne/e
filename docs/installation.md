# Installation

## Download a release (recommended)

Pre-built downloads are published for each release on the
[Releases page](https://github.com/kwhorne/e/releases).

### DMG (easiest)

Download **`e-<version>-universal.dmg`**, open it, and drag **e.app** into
**Applications**. It runs on both Apple Silicon and Intel Macs.

> The app is ad-hoc signed, so on first launch macOS may say it's from an
> "unidentified developer" — right-click the app → **Open** to allow it.

### Binary archive

For a CLI install, download the archive for your platform:

- **Apple Silicon (M1/M2/M3):** `e-aarch64-apple-darwin.tar.gz`
- **Intel Mac:** `e-x86_64-apple-darwin.tar.gz`

```sh
shasum -a 256 -c e-aarch64-apple-darwin.tar.gz.sha256   # optional
tar xzf e-aarch64-apple-darwin.tar.gz
mv e /usr/local/bin/e
```

Once installed, `e` keeps itself up to date — see [Updating](updating.md).

## Build from source

### Requirements

- [Rust](https://rustup.rs) 1.87 or newer
- A C toolchain (for tree-sitter grammars)
- Optional: language servers on your `PATH` (see [Languages & LSP](languages-and-lsp.md))

### Build

```sh
git clone https://github.com/kwhorne/e
cd e
cargo build --release
```

The binary is produced at `target/release/e`.

### Run

```sh
# Open a directory as a workspace
cargo run --release -- path/to/project

# Or open a single file
cargo run --release -- path/to/file.rs
```

### macOS app bundle

To build and launch a proper `.app` bundle (so the window comes to the front):

```sh
./scripts/run.sh path/to/project
```

To produce a distributable `.app` bundle:

```sh
./scripts/bundle-macos.sh
```

### Build a DMG installer

```sh
./scripts/bundle-dmg.sh             # host architecture
./scripts/bundle-dmg.sh --universal # universal (arm64 + x86_64)
```

This produces `dist/e-<version>.dmg` containing `e.app` and an `Applications`
symlink — open it and drag the app into Applications.

By default the app is **ad-hoc signed**, so on first launch macOS shows an
"unidentified developer" prompt; right-click the app → **Open** to allow it.

For distribution without that prompt you need an Apple Developer account:

1. Sign with a Developer ID certificate:
   ```sh
   CODESIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" ./scripts/bundle-dmg.sh --universal
   ```
2. Notarize the DMG with Apple and staple the ticket:
   ```sh
   xcrun notarytool submit dist/e-<version>-universal.dmg \
     --apple-id you@example.com --team-id TEAMID --password <app-specific-password> --wait
   xcrun stapler staple dist/e-<version>-universal.dmg
   ```

## Language servers

`e` uses the Language Server Protocol for rich language features. Install the
servers for the languages you use and make sure they are on your `PATH`:

| Language        | Server                        | Install |
| --------------- | ----------------------------- | ------- |
| PHP             | Intelephense                  | `npm i -g intelephense` |
| Rust            | rust-analyzer                 | `rustup component add rust-analyzer` |
| C / C++         | clangd                        | bundled with LLVM |
| TypeScript / JS | typescript-language-server    | `npm i -g typescript-language-server typescript` |
| Go              | gopls                         | `go install golang.org/x/tools/gopls@latest` |
| Python          | pyright                       | `npm i -g pyright` |

See [Languages & LSP](languages-and-lsp.md) for details.
