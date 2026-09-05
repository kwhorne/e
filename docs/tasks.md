# Tasks & tests

`e` detects runnable tasks from your project and runs them in the integrated
terminal.

## Run a task (`⌘⇧B`)

Opens the task palette with everything it found:

- **npm / yarn / pnpm / bun** scripts (from `package.json`, picking the right
  package manager from the lockfile)
- **Composer** scripts (`composer.json`)
- **Cargo** — `test`, `build`, `run`, `check`, `clippy`, `fmt`
- **Go** — `go test ./...`, `go build ./...`
- **Laravel** — `artisan test` / `serve` / `migrate` / `tinker`
- **Pest / PHPUnit**
- **Makefile** targets

Pick one (type to filter, arrows + Enter, or click) and it runs in a new,
named terminal tab so you can watch the output.

## Run tests

The **Run Tests** command (`⌘⇧P` → Run Tests) runs the project's primary test
command — `php artisan test`, `pest`, `phpunit`, `cargo test`, `go test ./...`,
or `npm test`, depending on what it detects.

## Grove

When [Grove](https://github.com/kwhorne/grove) is installed, a Laravel project
also gets `grove: dev start` / `grove: dev stop` (the site's Vite server, queue
worker and whatever the app declares via `DevCommands`), and — for a MySQL or
PostgreSQL database — **`artisan: migrate (snapshot first)`**, which takes a
`grove db snapshot` before running the migration so a bad one is one
`grove db restore` from undone, plus `grove: db snapshot` on its own.
