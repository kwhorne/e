# Laravel

`e` ships with PHP/Laravel-aware features on top of the
[Intelephense](languages-and-lsp.md) language server.

Inspired by the official Laravel VS Code extension, `e` introspects your project
(via `php artisan` and the filesystem) to provide completion, hover and
navigation for Laravel's helpers. It is enabled automatically in any project
with an `artisan` file; toggle it under **Settings → Laravel features** (or the
`laravel` config key). The data refreshes itself: saving a route, config, lang
or `.env` file in `e` reloads it at once, and a change on disk from outside —
`php artisan make:controller`, a `git checkout` — is noticed within a couple of
seconds. **Laravel: Refresh Project Data** (`⌘⇧P`) is still there for a forced
reload.

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

## Query-builder completion

Inside a query builder, column names complete from the model's table and the
live schema — `where('…')`, `orderBy()`, `select()`, `pluck()`, `value()`,
`groupBy()`, `having()` — and relationship names complete inside `with('…')`,
`load()`, `whereHas()`. The table is resolved from `Model::`, `$model`, or
`DB::table('…')`. Columns that don't exist in the schema are underlined with a
warning — a check PhpStorm can't do without the database.

## Laravel lints

From the same project data, `e` underlines what would fail at runtime, as you
type (300 ms after the last keystroke, off the UI thread):

| In | Check |
| -- | ----- |
| `view('…')`, `@include`, `@extends`, `@each`, `@component` | the view file exists (error) |
| `route('…')`, `to_route('…')` | the route is defined (error); its required `{parameters}` are passed (warning) |
| `__('file.key')`, `trans()`, `trans_choice()`, `@lang` | the key exists in `lang/` (warning; sentence keys are JSON translations that fall back to themselves) |
| `.env` | every key `.env.example` declares is set (warning at the top of the file) |

Arguments that are expressions (`'a.'.$b`, `$name`) and namespaced names
(`pkg::view`) are left alone. When the official `laravel/lsp` is running it
reports missing views and routes itself, so these lints stay off then.

## Package completions (`ide.json`)

Packages that ship a Laravel Idea [`ide.json`](https://laravel-idea.com/docs/ide_json/overview)
— and your own project's `ide.json` — teach `e` their string arguments: `new
Axis('…')` completes the strings the package lists, `->rule('…')` on its
validation class completes rule names, a function it declares as taking a route
name completes your routes. `e` reads `ide.json` from the project root and from
every `vendor/*/*/` on load, and honours the `completions` section: kinds
`routeName`, `viewName`, `configKey`, `translationKey`, `environmentVariable`,
`bladeComponent`, `validationRule`, `staticStrings`, `gate`/`policy`,
`inertiaPage`, bound by function, method, constructor, parameter position and
place (parameter or array value). Receiver types aren't inferred, so a
`->method('…')` rule matches by method name.

## Refactorings (code actions)

Alongside the language servers' quick fixes, the code-action picker offers
`e`'s own Laravel refactorings at the caret:

- **Convert validation string to array** — `'required|max:255'` →
  `['required', 'max:255']`.
- **Convert to `[Controller::class, 'method']`** — from the string
  `'UserController@index'` form.
- **Convert `{{ }}` ↔ `{!! !!}`** in Blade.
- **Convert `scopeActive()` to `#[Scope] active()`** (Laravel 12.6+), adding the
  `use Illuminate\Database\Eloquent\Attributes\Scope;` import when missing.

## Validation rules

Rule names complete inside `validate([…])` and FormRequest `rules()`. The
command **Laravel: Generate Validation Rules from Table** writes
`'field' => 'rules'` lines from the live schema (nullable → `nullable`,
`varchar(255)` → `max:255`, and so on) at the cursor.

## Gates & policies

`can()`, `authorize()`, `@can`, and `Gate::allows()` complete ability names and
jump (F12) to the policy method or `Gate::define()` that declares them.

## Generate model from table

With a table open in the database panel, **Laravel: Generate Model from Table**
creates an Eloquent model from the live schema — `$fillable`, `$casts`, and
`belongsTo`/`hasMany` relationships inferred from the real foreign keys.

## Event dispatch graph

`⌘⌥G` opens the event → listener graph, built from `$listen`, `Event::listen()`,
and auto-discovered `handle(EventType $event)` listeners. `F12` on a dispatched
event class jumps to a listener.

## Related files

`⌘⌥E` shows every file for the current resource — model, migration(s), factory,
seeder, controller, policy, request, resource, and test — in a quick picker.

## Livewire

`e` treats a Livewire component's class and Blade view as one unit:

- `wire:model="…"` completes from the component class's public properties.
- `F12` on a property in the view jumps to its declaration in the class;
  `⌘⌥J` switches between the view and the class.
- Renaming a property with `F2` updates **both** the class (`$prop`,
  `$this->prop`) and every `wire:` reference in the view.

## Runtime insight

`⌘⌥I` opens a continuous, Telescope-style panel that captures every request
against your dev app. No Telescope or Debugbar install required.

With [Grove](https://github.com/kwhorne/grove) serving the project, the panel
reads Grove's own request timeline — Grove is the proxy, so every request is
there with nothing installed in the app: method, path, status and duration.
Expanding a request fetches its **causal chain** from Grove: the SQL it issued
(turn on **SQL capture** in the panel header, which runs `grove sql-capture on`;
MySQL), the mail it sent, and the matching **error-log entries**. ✨ hands the
agent Grove's whole `explain` bundle — the request with credentials redacted, its
queries and mail, and the stacktrace from `laravel.log` — so it can go straight
to the cause. The replay base URL also comes from Grove (the real host, and
`http://` for a site without HTTPS).

Without Grove, the panel polls [Clockwork](https://underground.works/clockwork)
(`/__clockwork/latest`) as before: queries with N+1 warnings, cache hits/misses,
sent mails, and events.

**Verify the fix** (below) takes its query counts from Grove's chain too when
the app has no Clockwork — turn on SQL capture first.

### Mail and webhooks (Grove)

**Grove: Mail Catcher** (command palette) lists every email the app sent to
Grove's SMTP server — subject, recipients, time — and shows a message's text
when you click it; ✨ hands it to the agent to check content and find where it
is built. **Grove: Webhooks** lists the deliveries captured at
`/__grove/hooks/<bucket>`; select one and **↻ Re-deliver** sends the exact same
request to your handler at that path on the app's own URL, so you can fix the
handler and replay until it answers 200.

### Verify the fix (✓)

Click the **✓** on a captured request to verify a change end to end. `e`
checkpoints your working tree, replays the request and records a **baseline**
(time, query count, N+1). Apply your fix — edit the code or ask the agent — then
hit **Measure again**: `e` replays the request and shows a before/after
**verdict** (Improved / No change / Regressed / Broke). Keep the change, or
**Discard** it to revert to the checkpoint.

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
is what Grove serves the project as when Grove is running (else
`https://<folder>.test`); override it under **Settings → Laravel → App URL**.
"Explain with agent" hands the analysis to the AI panel.

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

## Pint and PHPStan

Both are picked up from `vendor/bin` — a project that doesn't use them gets
nothing, with no configuration and no behaviour change.

**Pint** becomes the formatter for PHP files. When the project ships
`vendor/bin/pint` it takes precedence over the language server, because a
Laravel project's formatting is whatever Pint says it is — that's what CI
enforces, and letting Intelephense format to its own taste would only produce a
diff for Pint to undo. It respects your `pint.json`: the buffer is formatted
through a temporary file *beside the original*, so a preset scoped to `app/`
sees the file as being in `app/`.

**PHPStan** runs on save, over the file you just saved rather than the whole
project, and its findings appear as warnings alongside the language server's.
Each carries PHPStan's rule identifier (`variable.undefined`) as the diagnostic
code, so you can look it up or baseline it. Larastan works too — it ships the
same binary.

It only runs when the project has both `vendor/bin/phpstan` and a config
(`phpstan.neon`, `phpstan.neon.dist` or `phpstan.dist.neon`), since PHPStan
needs the config to know its level and paths. Findings cover the whole line:
PHPStan reports a line but no column, and guessing a span would put the squiggle
under the wrong token.

If PHPStan itself fails — a broken config, a path that doesn't exist — the error
is reported rather than swallowed. A run that never looked at your code must not
be mistaken for a clean one.

## Test results

Press `⌘⇧T` for the test panel, or **Run tests** from the
[ship gate](source-control.md#ship-it).

Where the runner can write JUnit XML — `php artisan test`, Pest, PHPUnit,
Vitest — the panel lists each failing test with its first assertion line, and
clicking one opens the file at the failing line. The toolbar shows
`12 passed · 2 failed · 1 skipped`, because *the suite failed* and *two of ninety
tests failed* are different news.

Runners that can't produce a report are run exactly as before, with their plain
output. Nothing is passed a flag it doesn't take.

> PHPUnit reports a failure's location only in the message text, not as an
> attribute, so a failure it can't describe that way is listed without a link
> rather than pointed somewhere wrong.
