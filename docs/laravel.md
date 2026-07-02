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

## Livewire

`e` treats a Livewire component's class and Blade view as one unit:

- `wire:model="…"` completes from the component class's public properties.
- `F12` on a property in the view jumps to its declaration in the class;
  `⌘⌥J` switches between the view and the class.
- Renaming a property with `F2` updates **both** the class (`$prop`,
  `$this->prop`) and every `wire:` reference in the view.

## Runtime insight

`⌘⌥I` opens a continuous, Telescope-style panel that captures every request
against your dev app via [Clockwork](https://underground.works/clockwork):
method, URI, status and duration, plus SQL queries with N+1 warnings, cache
hits/misses, sent mails, and events. Click a request to expand its queries; ✨
hands it to the agent. No Telescope or Debugbar install required.

For Inertia/VILT projects, see [Inertia & the VILT stack](inertia.md).

## Tinker scratchpad

Press `⌘⌥T` for a Tinker scratchpad: write PHP and press `⌘↵` to run it against
your app via `php artisan tinker`, with the output shown below. Select code in
the editor and run **Tinker: Run Selection** to evaluate it. The AI agent can
also write and run Tinker snippets over the sync socket.

## Architecture map

`⌘⌥M` opens an interactive map of your routes: each row shows
route → controller → views as clickable cards. Click the controller to jump to
its method, or a view to open the Blade file. Filter by route name, URI or
action.

## Eloquent completion (live schema)

When you type `$user->` on a model instance, `e` suggests the model's real
database columns — read from the live schema (via `.env`) at startup. It infers
the model from `$var = Model::…` or a type hint, maps it to its table
(`protected $table` or the snake_case + plural convention), and merges the
columns alongside Intelephense — something Intelephense can't do on its own.

## Relationship graph

`⌘⌥R` parses the relationships from your models (`hasMany`, `belongsTo`,
`belongsToMany`, `morph*`) and cross-checks them against the **live database's
foreign keys**. Each model is a node; click a relationship to jump to the related
model or method. Relations that exist in code but have no backing foreign key are
flagged ⚠ — so alongside the schema diff you see code, migrations, and the actual
database in one place.

## Security lens

In the architecture map (`⌘⌥M`) every route shows its middleware stack and a
badge: 🔒 when it's authenticated, ⚠ when a state-changing route (POST/PUT/PATCH/
DELETE) has no authentication. The header counts unprotected routes, and clicking
a ⚠ asks the agent to suggest the right middleware/policy.

## Generate a test from a replay

After replaying a route (▶ in the map), the **🧪 Test** button writes a Pest
feature test to `tests/Feature/` using the request path, the response status, and
assertions inferred from the response (JSON structure or an HTML `<title>`). It
opens the file, ready for the `⌘⇧T` "fix to green" loop.

## Schema diff

**Laravel: Schema Diff** (command palette) compares your migrations against the
live database and lists discrepancies — columns that exist in the DB but no
migration creates, and columns a migration adds that aren't in the DB yet.

## Log tail

`⌘⌥L` opens a live tail of `storage/logs/laravel.log`: levels are coloured,
stack-trace frames are clickable (jump to file:line), and **Fix with AI** hands
the latest error to the agent.

## Request replay

In the architecture map (`⌘⌥M`), click ▶ on a GET route to replay the request
against your running app and see the response — plus the SQL queries it ran
(if the app has `laravel/clockwork`), with N+1 duplicates flagged. The base URL
defaults to `https://<folder>.test` (Grove); override it under
**Settings → Laravel → App URL**. "Explain with agent" hands the analysis to the
AI panel.

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
