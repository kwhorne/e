# Laravel

`e` ships with PHP/Laravel-aware features on top of the
[Intelephense](languages-and-lsp.md) language server.

Inspired by the official Laravel VS Code extension, `e` introspects your project
(via `php artisan` and the filesystem) to provide completion, hover and
navigation for Laravel's helpers. It is enabled automatically in any project
with an `artisan` file; toggle it under **Settings → Laravel features** (or the
`laravel` config key). Run **Laravel: Refresh Project Data** from the command
palette (`⌘⇧P`) after adding routes, views or config.

## Helper completion

When the caret is inside one of Laravel's helpers, `e` offers completions sourced
from your project:

| Helper                         | Completes |
| ------------------------------ | --------- |
| `route('…')`                   | named routes (with method + URI) |
| `view('…')`                    | Blade view names |
| `config('…')`                  | config keys (with resolved value) |
| `env('…')`                     | environment variables (with value) |
| `__('…')`, `trans('…')`, `@lang` | translation keys (with text) |
| `<x-…>`                        | Blade components |

These are read from your project, so they reflect your actual routes, views,
config, `.env`, language files and components.

## Hover & go to definition

- **Hover** (`F1`) over a helper string shows the resolved value — a config
  value, a route's method/URI/action, an env value, or a translation's text.
- **Go to definition** (`F12`) jumps to the target:
  - `route('…')` → the controller method
  - `view('…')` → the Blade file
  - `config('…')` → the config file (and the key's line)
  - `env('…')` → the `.env` line
  - `__('…')` → the language file
  - `<x-…>` → the component's Blade file

## Blade templates

`*.blade.php` files are detected as **Blade** with full syntax highlighting:
HTML tags, attributes and Tailwind utility classes, Blade directives
(`@php`, `@if`, `@foreach`, `@push`, …), `{{-- comments --}}`, and the embedded
PHP inside `@php … @endphp` blocks, `{{ … }}` and `{!! … !!}` expressions.

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
