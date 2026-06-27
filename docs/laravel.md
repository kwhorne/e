# Laravel

`e` ships with PHP/Laravel-aware features on top of the
[Intelephense](languages-and-lsp.md) language server.

## Helper completion

When the caret is inside one of Laravel's string helpers, `e` offers completions
sourced from your project:

| Helper       | Completes |
| ------------ | --------- |
| `route('…')` | named routes |
| `view('…')`  | Blade view names |
| `config('…')`| config keys |
| `env('…')`   | environment variables |

These are read from your project, so they reflect your actual routes, views,
config, and `.env`.

## Blade templates

`*.blade.php` files are detected as **Blade** and highlighted using the HTML
grammar, which covers tags, attributes, and Tailwind utility classes alongside
Blade directives.

## Working on a Laravel project

1. Open the project root: `e ~/code/my-laravel-app`.
2. Ensure `intelephense` is installed and on your `PATH`.
3. Use `⌘P` to jump between controllers, models, and views; `⌘⇧O` to find
   classes and methods; and the [Source Control panel](source-control.md) for
   commits.

## Tips

- The [AI agent panel](agents.md) (`⌘L`) is handy for Laravel scaffolding and
  refactors — point it at your project and let it work alongside you.
- Use [workspace search](find-and-replace.md) (`⌘⇧F`) to find usages across
  Blade views and PHP classes at once.
