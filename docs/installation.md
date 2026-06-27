# Installation

## Download a release (recommended)

Pre-built binaries are published for each release on the
[Releases page](https://github.com/kwhorne/e/releases).

1. Download the archive for your platform:
   - **Apple Silicon (M1/M2/M3):** `e-aarch64-apple-darwin.tar.gz`
   - **Intel Mac:** `e-x86_64-apple-darwin.tar.gz`
2. Verify the checksum (optional):
   ```sh
   shasum -a 256 -c e-aarch64-apple-darwin.tar.gz.sha256
   ```
3. Extract and move the binary onto your `PATH`:
   ```sh
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

To produce a distributable bundle:

```sh
./scripts/bundle-macos.sh
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
