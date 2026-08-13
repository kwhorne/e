# Debugging

`e` has a built-in step debugger: breakpoints, a live call stack, variable
inspection and step controls. It speaks the [Debug Adapter Protocol
(DAP)](https://microsoft.github.io/debug-adapter-protocol/), so the same UI
drives every DAP-capable language:

| Language            | Adapter              | Notes                                    |
| ------------------- | -------------------- | ---------------------------------------- |
| PHP                 | `vscode-php-debug`   | via Xdebug (pairs with [Grove](#php--xdebug-with-grove)) |
| JavaScript / TypeScript | `vscode-js-debug` | Node programs                            |
| Rust / C / C++      | `codelldb`           | needs a compiled executable              |

The adapter is chosen automatically from the active file's language.

## Quick start

1. Open a file and set a breakpoint — press `F9` on a line, or `⌥`-click it.
   A red dot appears in the editor margin.
2. Press `F5` to start. The Debug panel opens and shows the session status.
3. When execution hits a breakpoint, the line is highlighted, the editor jumps
   to it, and the call stack + variables fill in.
4. Step through with `F10` / `F11` / `⇧F11`, or `F5` to continue.

## Controls

| Shortcut | Action |
| -------- | ------ |
| `F5`     | Start / continue |
| `F9`     | Toggle breakpoint on the caret line |
| `⌥`-click | Toggle breakpoint on the clicked line |
| `F10`    | Step over |
| `F11`    | Step into |
| `⇧F11`   | Step out |

The Debug panel (also reachable via **Debug: Toggle Panel** in the command
palette, `⌘⇧P`) shows execution controls, the call stack (click a frame to jump
to its source), the current frame's variables, and all breakpoints. **Debug:
Stop** ends the session.

## PHP / Xdebug with Grove

PHP step-debugging needs two things: Xdebug loaded into the PHP that runs your
app, and the `vscode-php-debug` adapter to translate DAP into Xdebug's DBGp
protocol.

[Grove](https://elyracode.com/grove) supplies both:

1. Enable Xdebug in your local runtime. Either flip **Enable Xdebug** in
   Settings (`⌘,` → Laravel), which runs `grove debug on` for you, or from a
   terminal:

   ```sh
   grove debug on
   ```

   Grove loads Xdebug into its FPM pools in *trigger* mode (near-zero overhead
   until a request opts in). If your bundled PHP has no Xdebug, install a
   debug-enabled build with `grove debug install <version>`.

   > The Settings toggle reflects Grove's real state (it's synced from
   > `grove debug status` on startup) and degrades gracefully: if Grove isn't
   > installed it just reports so in the Debug panel — the editor and its other
   > adapters are unaffected. Debugging is entirely opt-in; `e` runs normally
   > without Grove or any adapter installed.

2. In `e`, set breakpoints and press `F5`. The adapter is launched over Grove's
   bundled Node automatically, and Xdebug connects to it on port 9003.

3. Trigger a request that hits your breakpoint — use a browser Xdebug-helper
   extension, add `?XDEBUG_TRIGGER=1`, or for CLI (artisan, tests) run:

   ```sh
   eval "$(grove debug env)"
   php artisan ...
   ```

## PHP in Docker or Laravel Sail

When PHP runs in a container, Xdebug reports the paths *it* sees —
`/var/www/html/app/Models/User.php` — which exist nowhere on your machine. The
adapter has to translate them back, or breakpoints simply never bind: no error,
no stop, nothing.

`e` reads `docker-compose.yml` (or `docker-compose.yaml` / `compose.yml`) and
finds the bind mount of your project root, so a stock Laravel Sail setup works
with no configuration:

```yaml
services:
    laravel.test:
        volumes:
            - '.:/var/www/html'      # ← found automatically
```

Named volumes and unrelated bind mounts are ignored; only a mount of the project
root itself is used. Both the short syntax above and the long
`source:`/`target:` form are understood.

For anything that can't be inferred — a remote interpreter over SSH, a mount
defined outside compose — set the mapping yourself as JSON, remote path first:

```sh
E_PHP_PATH_MAPPINGS='{"/srv/app":"/Users/you/code/app"}' e .
```

A project with no compose file gets no mapping, which is correct for a native
interpreter.

## Adapter discovery

`e` finds adapters from installed VS Code / Cursor extensions automatically. To
point at a specific install, set an environment variable:

| Variable                | Purpose                                         |
| ----------------------- | ----------------------------------------------- |
| `E_PHP_DEBUG_ADAPTER`   | Path to `phpDebug.js` (`vscode-php-debug`)       |
| `E_JS_DEBUG_ADAPTER`    | Path to `dapDebugServer.js` (`vscode-js-debug`)  |
| `E_CODELLDB`            | Path to the `codelldb` binary                    |
| `E_DEBUG_PROGRAM`       | Executable to launch (Rust/C/C++)                |
| `E_JS_DEBUG_PORT`       | TCP port for the js-debug server (default 8123)  |
| `E_CODELLDB_PORT`       | TCP port for codelldb (default 9552)             |

Node itself is discovered from Grove's runtimes, falling back to `node` on your
`PATH`.

## Troubleshooting

- **"No php-debug adapter found"** — install the *PHP Debug* extension in
  VS Code/Cursor, or set `E_PHP_DEBUG_ADAPTER`.
- **Breakpoints never hit (PHP)** — check `grove debug on` is active and that the
  request actually carries an Xdebug trigger (cookie/param or `grove debug env`).
- **Rust: "Set E_DEBUG_PROGRAM"** — build first (`cargo build`); `e` looks for
  `target/debug/<crate>`, or set `E_DEBUG_PROGRAM` to the executable.
