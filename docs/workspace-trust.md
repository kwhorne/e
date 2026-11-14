# Workspace trust

Opening a folder doesn't run anything its author wrote. Until you trust a
project, `e` reads it but doesn't run it.

## What works in restricted mode

Editing, highlighting, search, navigation, the file tree, git, the terminal,
the database panel, and the language servers that only read your code
(Intelephense, clangd, Pyright).

## What waits until you trust the project

- `php artisan` and anything else that loads the project's PHP: the routes,
  translations and config `e` reads from Laravel, `migrate --pretend` on a
  migration, inline evaluation (`⌘↵`), Tinker, and the Artisan palette.
- The project's own tools in `vendor/bin`: Pint and PHPStan on save.
- Tests (on save, from the test panel and the TDD loop), tasks and the
  debugger.
- Language servers that load project code: the TypeScript server (which
  uses the project's `node_modules/typescript` and its plugins),
  rust-analyzer (`build.rs`, proc macros), gopls (the Go toolchain named in
  `go.mod`) and laravel-lsp (which boots the app).
- `.e/rules.toml` given to an agent as instructions. The rules are still
  checked as you type.

A banner above the editor says what is held back. **Trust Project** on the
banner, or **Trust Project** in the command palette (`⌘⇧P`), trusts it for
good and starts what was waiting. **Restrict Project** takes trust back.
Anything already running keeps running until `e` restarts.

## Where trust is kept

Trusted folders are listed in `~/.config/e/trusted.json`:

```json
{ "trusted": ["/Users/you/Code/shop"] }
```

A folder inside a trusted folder is trusted. A git worktree of a trusted
repository is trusted too (agents and Grove make those), but only when the
repository itself has registered the worktree.

The first time a version of `e` with workspace trust runs, the projects in
your recent list are trusted, since their code has already run. The folder it
is opening isn't, unless it was already on that list.
