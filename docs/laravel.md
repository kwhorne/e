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
| `<x-alert …`                   | the component's props as attributes (from `@props([…])` or the class's constructor) |
| `@include`, `@extends`, `@each`, `@component` | view names (with hover and go to definition) |

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
  - `__('…')` → the key's line in the language file, in the app's locale
    (nested keys and JSON files included)
  - `<x-…>` → the component's Blade file, or its class in
    `app/View/Components` for a class-based component
  - `can('update', $post)`, `@can`, `Gate::allows` → the method on the policy
    for the model in the second argument (`PostPolicy::update`)
  - an event class → the class itself; from where it is dispatched
    (`event(new …)`, `…::dispatch(`), its listener

## Find Usages and Rename for keys

The keys Laravel resolves at runtime — route names, view names, translation
keys — are strings no PHP language server follows. On one, **Find Usages**
(`⇧F12`) lists every place it appears across PHP, Blade and, for routes,
Ziggy's `route()` in your JS, declarations first: the `->name('…')`, the view
file, the key's line in each language. In a Blade view with no key under the
caret, it answers *who renders this view?*

**Rename** (`F2`) on such a key rewrites all of them through the usual preview,
and for a view moves its file to match. It refuses what it couldn't finish: a
route name built from a group's `Route::name('admin.')` prefix (rename it
where it is declared), or a translation key whose group would change (that
would move it to another file).

## Blade templates

`*.blade.php` files are detected as **Blade** with full syntax highlighting:
HTML tags, attributes and Tailwind utility classes, Blade directives
(`@php`, `@if`, `@foreach`, `@push`, …), `{{-- comments --}}`, and the embedded
PHP inside `@php … @endphp` blocks, `{{ … }}` and `{!! … !!}` expressions.
`<script>` blocks are JavaScript and `<style>` blocks CSS.

## Query-builder completion

Inside a query builder, column names complete from the model's table and the
live schema — `where('…')`, `orderBy()`, `select()`, `pluck()`, `value()`,
`groupBy()`, `having()` — and relationship names complete inside `with('…')`,
`load()`, `whereHas()`. The table is resolved from `Model::`, `$model`, or
`DB::table('…')` — and inside a closure from what the closure queries:
`whereHas('posts', fn ($q) => $q->where('…'))` completes `posts` columns
(dotted relations followed), a grouped `where(fn ($q) => …)` stays on the outer
table, and a closure that can't be told is left alone. Columns that don't exist
in the schema are underlined with a warning — a check PhpStorm can't do without
the database.

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

Two of them come with a fix in the code-action picker: **Create
resources/views/…blade.php** for a missing view (the file opens once created),
and **Add `key` to lang/<locale>/file.php** for a missing translation key, where
the locale is `APP_LOCALE` from `.env` (else `app.locale`, else `en`) and the
file is created around the key when it doesn't exist yet.

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
- **Promote to FormRequest** — with the caret in `$request->validate([…])` (or
  `$this->validate($request, […])`): creates `app/Http/Requests/StoreUserRequest.php`
  (named from the method and controller) holding the rules, replaces the call
  with `$request->validated()`, retypes the method's `Request` parameter and adds
  the import. The new class opens.

## Validation rules

Rule names complete where rules are written — FormRequest `rules()`, a
`$rules` property, `validate([…])`, `Validator::make()` and Livewire's
`#[Validate(…)]` — and not after any `=> '` elsewhere. So do
the tables and columns `exists:` and `unique:` name — `exists:us` offers your
tables, `exists:users,` their columns, from the live schema; `Rule::exists('…')`
and `Rule::unique('users', '…')` likewise. The command **Laravel: Generate
Validation Rules from Table** writes `'field' => 'rules'` lines from the live
schema (nullable → `nullable`, `varchar(255)` → `max:255`, and so on) at the
cursor.

## Tests: run the one at the caret

`⌘⌥⇧T` runs only the test the caret is in — a Pest `it('…')` / `test('…')`
description or a PHPUnit `test_…` / `#[Test]` method — as `php artisan test
<file> --filter="…"`, in the TDD panel, so a failure still has a line to jump to.
The next `⌘⇧T` runs the whole suite again.

## The Laravel menu

`⌘⇧,` opens the command palette filtered to `Laravel: ` — every Laravel command
in one list, the way Laravel Idea's menu gathers them: code generation, the
Artisan palette, route search, the Eloquent helper, migrations, the maps and
panels. Type to narrow it further.

## Code generation

**Laravel: New Eloquent Model…** (`⌘⇧N`) is a spec in an editor panel with a
live preview of the files it will create. Fields have a type, `?` for nullable
and an optional default; `foreignId Customer` is a foreign key *and* a
`belongsTo`. Relations, options (`id timestamps softDeletes fillable`, prefix
`-` to turn one off) and what to generate are each one line:

```
model: Order
fields:
  customer_id   foreignId Customer
  status        string = 'pending'
  total         decimal(10,2)
  notes         text?
relations:
  hasMany OrderLine
options: id timestamps fillable
generate: migration factory seeder request controller api-controller resource policy
```

`⌘↵` (or **Create**) writes the model with `$fillable`, `casts()` and the
relation methods, plus whatever `generate:` lists: the `create_orders_table`
migration; a factory with fakes chosen from the column types and names; a
seeder; `StoreOrderRequest`/`UpdateOrderRequest` with rules derived from the
fields (`required|string|max:255`, `nullable|string`, `exists:customers,id`,
`sometimes` on update); a resource controller that uses those requests (or an
API one under `App\Http\Controllers\Api` returning the JSON resource);
`OrderResource`; `OrderPolicy`. Files that already exist are left alone and
listed. The language servers are told about the new files at once, so they
complete immediately.

**Laravel: New Pivot Table…** is the same panel with a `pivot: Post Tag` line:
one migration for `post_tag` with both foreign keys, a composite primary key
and (unless `options: -timestamps`) timestamps.

**Laravel: New Eloquent Model from Database Table** builds the model from the
live schema instead (see below). **Laravel: New Class…** is the Artisan palette
opened on `make:` — every generator the app and its packages declare
(controller, job, event, listener, mail, notification, command, Livewire
component, …), with Artisan's own usage line as the hint for the arguments.

## Artisan

`⌘⇧A` opens the Artisan palette with every command the app declares (its own
and its packages'), `make:` first. Pick one, and the usage line becomes the hint
for an argument line: `Order -mf` for `make:model`, `--seed` for `migrate`.
A `make:` command runs in the background and opens what it created — the model
and, with `-mf`, its migration and factory. Anything else runs in a terminal
tab, so prompts and output work as in a shell.

## Eloquent helper

**Laravel: Generate Eloquent Helper** writes `_ide_helper_models.php` in the
project root: for every model, `@property` per column with its PHP type from the
live schema, `@property-read` for each relationship (a `Collection<int, Post>`
plus `posts_count` on the many side), and `@method static Builder` for each
scope (`scopeActive()` or `#[Scope]`). Intelephense reads it like any file, so
`$user->posts`, `$order->total` and `User::active()` complete and type-check
everywhere — in closures, on collections, in Blade. Once the file exists it is
regenerated whenever the schema is reloaded, and the language servers are told
about it at once. Add it to `.gitignore`.

## Routes: controller actions

In a route file, `[UserController::class, '` completes the controller's public
methods, resolving the class through the file's `use` imports and Composer's
PSR-4 map.

## Route search

`⌘⇧R` lists every route, named or not — `GET /users/{user}` with its name and
action — and filters as you type on method, URI, name or controller. Enter
opens the controller action, or the routes-file line for a closure route (also
inside a `Route::prefix()` group).

## Find unused views

**Laravel: Find Unused Views** scans `app/`, `routes/`, `config/` and the views
themselves for `view()`, `@include`/`@extends`/`@each`/`@component`,
`Route::view()`, `View::make()`, mailables' `->view()`/`->markdown()`,
`->layout()`/`#[Layout]`, `'view' =>`/`'layout' =>` entries and `<x-…>` tags,
and lists the Blade views nothing refers to. Livewire views and single-file
components, `errors.*` and `vendor.*` are resolved by convention and left out.
A view named from a variable can't be seen, so read the list as "worth a look";
Enter opens one.

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

`e` treats a Livewire component's class and Blade view as one unit — in all
three shapes a component can take:

- **class-based**: `app/Livewire/Admin/UserTable.php` with
  `resources/views/livewire/admin/user-table.blade.php`;
- **single-file** (Livewire 4, and Volt on Livewire 3): one Blade file whose
  `<?php … ?>` block holds `new class extends Component { … }`, such as
  `resources/views/components/⚡counter.blade.php`;
- **multi-file** (Livewire 4): `⚡counter/counter.php` beside
  `⚡counter/counter.blade.php`.

Then:

- `wire:model="…"` completes from the component's public properties — in a
  single-file component from the buffer you're editing, unsaved changes and all.
- `F12` on a property in the view jumps to its declaration;
  `⌘⌥J` switches between a view and its class.
- Renaming a property with `F2` updates **both** the class (`$prop`,
  `$this->prop`) and every `wire:` reference and `$prop` in the view — in one
  edit when they share a file.
- `wire:click="…"`, `wire:submit`, `wire:change` and the other event attributes
  complete from the component's action methods (public, minus lifecycle hooks).
- `<livewire:…>` and `@livewire('…')` complete every component by the name
  Livewire knows it by — `admin.user-table`, `counter`, `pages::dashboard`
  (Livewire 4's `pages::` and `layouts::` namespaces; Volt pages unprefixed on
  Livewire 3, read from `composer.lock`) — and `F12` on one, or on
  `Route::livewire('/', 'pages::dashboard')`, opens it.

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
to the cause. 🧪 writes a feature test from the request
([below](#a-test-from-a-request)). The replay base URL also comes from Grove
(the real host, and `http://` for a site without HTTPS).

Without Grove, the panel polls [Clockwork](https://underground.works/clockwork)
(`/__clockwork/latest`) as before: queries with N+1 warnings, cache hits/misses,
sent mails, and events.

**Verify the fix** (below) takes its query counts from Grove's chain too when
the app has no Clockwork — turn on SQL capture first. Grove 1.12.4 or later is
needed for it to see a Laravel app's queries; older versions missed PDO's
prepared statements.

### Runtime lenses

What the running app just did shows at the end of the line of the controller
method that did it:

```php
public function index(Request $request)   ⚡ 42 ms · 14 queries · N+1 orders ×12 · 2m ago
```

Each captured request is matched to the route the router would pick, and the
route to its action; the newest request per action is the one shown, with the
number of requests when there were several. A Livewire page or an invokable
controller gets its lens on the `class` line. The lens turns yellow for an N+1
or a request over a second, and red for a server error. Queries are shown only
when they could be seen — Clockwork in the app, or Grove with SQL capture on —
never as a guessed zero.

`⌘.` on the line offers **Runtime: show** (the request, expanded, in the Runtime
panel) and **Runtime: ask the agent to fix … and measure it**: the agent gets
the request with its queries and stacktrace, and is told to measure the route
with [`verify_route`](agent-sync.md#measuring-a-route) before and after its
change and to report that verdict.

With Grove, the timeline is read every two seconds while a file under `app/` is
open, so the lenses follow your browser without the Runtime panel open; with
Clockwork, they follow while the panel is open. Turn them off under
**Settings → Laravel → Runtime lenses**.

### A test from a request

Click through something in the browser, then 🧪 on its row in the Runtime
panel (or **Runtime: write a test from …** under `⌘.` on a lens): `e` writes
the feature test that replays it, in the project's style, and opens it.

```php
test('PUT /orders/{order}', function () {
    $user = User::factory()->create();
    $order = Order::factory()->create(['user_id' => $user->id]);

    $this->actingAs($user)
        ->put(route('orders.update', $order), [
            'status' => 'paid',
            'note' => 'Leave at the door',
        ])
        ->assertSessionHasNoErrors()
        ->assertRedirect();

    $this->assertDatabaseHas('orders', ['id' => $order->id, 'status' => 'paid']);
});
```

- **Pest or PHPUnit**, whichever the project uses. The test goes in
  `tests/Feature/` under the controller's name (`Admin/OrderControllerTest.php`),
  at the end of the file when there is one, with the imports it lacks.
  A new file gets `RefreshDatabase` unless the project's tests already reset the
  database.
- **The route's models** are made with their factories, owned by the signed-in
  user when they belong to one (`user_id`). A `*_id` field in the body that
  names a model gets one made too. A route behind `auth` signs a user in, on the
  route's guard (`actingAs($user, 'sanctum')`).
- **The status** becomes `assertOk()`, `assertRedirect()`, `assertNoContent()`…;
  a form that redirected also asserts it came back without validation errors.
  A request that **failed** becomes a test that expects it to succeed — red
  until the bug is fixed.
- **What it wrote**, with Grove's SQL capture on, becomes `assertDatabaseHas`,
  `assertSoftDeleted` or `assertModelMissing` — only for values the request
  sent, never ones the app made up such as ids and timestamps.
- **A Livewire update** becomes `Livewire::test('name', [models])->set(…)->call(…)->assertHasNoErrors()`.
- A secret Grove redacted is sent as `'password'` (the factory's) or `'secret'`.

What the test could not set up — a model without a factory, one that belongs to
a team — is in the notification. So is a warning when `phpunit.xml` names no
test database of its own, since `RefreshDatabase` would then empty the app's.

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
database columns — read from the live schema (via `.env`). It infers the model
from `$var = Model::…` or a type hint (not from `->get()`, `->paginate()` or
`->count()`, which aren't one model), maps it to its table (`protected $table`
or the snake_case + plural convention), and merges the columns alongside
Intelephense — something Intelephense can't do on its own.

**In Blade** the variable comes from where the view is rendered:
`view('users.show', compact('user'))` with a type-hinted `User $user`,
`['user' => $user]`, `->with('user', …)`, `auth()->user()`. A Livewire view
takes its component's typed properties.

The schema follows your migrations: a local database is checked every ten
seconds and re-read when its `migrations` table changes — however the
migration ran — and open PHP files are re-linted, so a column you just added
is no longer "unknown".

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

### Unchecked lookups (IDOR)

A route behind `auth` with a `{parameter}`, and a controller method that turns
it into a model — `Order $order`, or `Order::findOrFail($id)` — with nothing that
asks whether this user may have that order: any signed-in user can then read or
change anyone's by changing the id in the URL. `e` warns on the method's line:

```text
GET orders/{order} hands any signed-in user the Order {order} they ask for, and
it belongs to someone (user_id): nothing here checks it is theirs …
```

It errs towards silence. Only a model that belongs to someone counts — a
`user_id`, `team_id`, `owner_id`… or a `belongsTo(User::class)` (not
`'created_by'`, which says who made the row) — and not one a global scope
already confines, such as a tenant or team trait. Anything that looks like a
check silences it: `authorize()`, `Gate`, `->can()`, a FormRequest whose
`authorize()` does more than `return true`, `can:`/role/admin/signed middleware,
`authorizeResource()` in the controller, scoping to the user
(`$request->user()->orders()->findOrFail(…)`, `auth()->id()`, `whereBelongsTo`),
or a helper like `$this->ensureOwns($order)`. Slugs and tokens in the URL are
left alone. Run over 81 real projects, it raised 11 warnings in 3 of them.

The same check flags a method a changeset touches in
[session review](source-control.md#session-review-v), and marks the route
`"unchecked"` in the agent's [`laravel_map`](agent-sync.md#the-laravel-project-in-one-answer).

## Generate a test from a replay

After replaying a route (▶ in the map), the **🧪 Test** button writes a Pest
feature test to `tests/Feature/` using the request path, the response status, and
assertions inferred from the response (JSON structure or an HTML `<title>`). It
opens the file, ready for the `⌘⇧T` "fix to green" loop.

## Migrations

**Laravel: Migrations** (command palette) lists every migration with its batch
and whether it has run, from `php artisan migrate:status`. **▶ Migrate** runs
what's pending — behind a `grove db snapshot` when Grove serves the database, so
a bad migration is one `grove db restore` from undone — and **↶ Rollback last
batch** runs `migrate:rollback --step=1`; both in a terminal tab, with the list
re-read once they have had a moment to finish. Click a migration to open its
file.

### Before a migration runs

Open or save a migration that hasn't run, and `e` asks `php artisan migrate
--pretend` what SQL it will run. Each statement shows dimmed at the end of the
line that produces it:

```php
$table->string('phone');   ⇂ alter table `users` add `phone` varchar(255) not null
```

Then the statements are checked against the data already in your local
database, and what would go wrong is marked on the line:

| | |
| --- | --- |
| A `NOT NULL` column without a default, on a table with rows | **error** on Postgres, which refuses it; on SQLite always, even on an empty table; on MySQL for a date or time column (strict mode refuses the zero date). On MySQL otherwise a **warning**: each row gets `''` or `0`. |
| `->change()` to `NOT NULL` while rows hold `NULL` | **error**, with how many rows |
| `->change()` to a shorter length than values already have | **error**, with how many rows |
| A unique index over values that occur more than once | **error** |
| A foreign key that rows have no parent for | **error** |
| `dropColumn` / `drop` on data | **warning**: `down()` can bring back the column or table, not what was in it |
| A type change on a table with rows | **info**: it rewrites the table, which on a large production table holds a lock |

Only counts are run against the database, and nothing when the `.env`
connection looks like production. A migration that has already run is left
alone.

On Postgres and SQLite, `⌘.` in a migration also offers **Migration: dry-run
up() and down(), rolled back**. `e` runs `up()` then `down()` (for one that has
run, `down()` then `up()`) inside a transaction it then rolls back, so the
database is left exactly as it was; a method that throws gets the error on its
line. MySQL commits schema changes as it makes them, so there it isn't offered.
Code in a migration that reaches outside the database — files, queues, HTTP —
does run, and isn't undone.

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
