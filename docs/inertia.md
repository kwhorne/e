# Inertia & the VILT stack

`e` understands both sides of an Inertia app — Laravel/PHP on one side, your
Vue/Svelte/React page components on the other — and the bridge between them.

## Page resolution

`Inertia::render('Users/Index')` behaves like `view()` does:

- **Go to definition** on the page name jumps to the component under
  `resources/js/Pages/Users/Index.{vue,tsx,jsx,ts,js,svelte}`.
- **Completion** inside `Inertia::render('…')` suggests existing page components.
- The **architecture map** (`⌘⌥M`) goes route → controller → **page component**,
  not just route → controller.

## The props contract (`⌘⌥C`)

The controller sends props; the page component just hopes the shape is right.
`e` reconciles the two because it understands PHP, TypeScript, *and* the
database at once.

Open a page component and press `⌘⌥C`. `e` finds the controller that renders it,
parses the props, and shows:

- **Inferred types** — `User::paginate()` becomes `User[]`, whose fields come
  from the live database schema; `find()`/`first()` become `User`; scalars,
  arrays and booleans are inferred too.
- **Props sent but never used** in the component (⚠ amber).
- **Props the component expects but the controller never sends** (⚠ red) —
  shared props are taken into account.
- **Form contract** — `useForm({ … })` fields checked against the matching
  FormRequest's `rules()` (following `form.post(route('…'))` → controller →
  FormRequest): fields that aren't validated, and rules with no field.

**Generate TypeScript** writes interfaces to `resources/js/types/<Page>.d.ts`,
expanding each model into an interface built from the real database columns
(nullable columns become optional). No `spatie/typescript-transformer`,
Wayfinder, or typegen script required.

## Ziggy routes on the JS side

`route('users.show')` in JS/TS/Vue/Svelte gets completion, hover (method + URI +
action) and go-to-definition (to the controller) — from the same route table the
PHP side uses. This covers `<Link :href="route('…')">` too.

## Shared props

`HandleInertiaRequests::share()` is parsed so global props like
`$page.props.auth.user` complete everywhere, one nested level deep.

## Inertia-aware replay

Replaying an Inertia route (▶ in the architecture map) shows the response as an
explorable **props tree** — the component name at the top (click to open it) and
the props laid out as a tree — instead of the raw HTML. N+1 detection still works
on the captured queries.
