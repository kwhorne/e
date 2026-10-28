# AI Agent Workspace Sync

CLI agents (Elyra, Claude Code, Codex, …) run in the [agent panel](agents.md),
but on their own they only see files on disk — not what you're doing in the
editor. The **workspace sync** closes that gap with a small local socket the
agent (or any tool) can talk to.

When the editor starts it opens a per-process Unix socket and exports its path
to spawned agents as **`$E_EDITOR_SOCK`**. The protocol is one JSON object per
line, with one JSON response per line.

## Reading editor state

```sh
printf '{"method":"context"}\n' | nc -U "$E_EDITOR_SOCK"
```

Returns the current file, cursor (1-based `line`/`col`), the selected text, the
language, the dirty flag, the list of open files, the workspace `root`, and all
diagnostics:

```json
{
  "ok": true,
  "root": "/path/to/project",
  "file": "/path/to/project/app/Models/User.php",
  "line": 42, "col": 9,
  "selection": "User::query()",
  "language": "Php",
  "dirty": true,
  "open_files": ["…"],
  "diagnostics": [
    {"file": "…", "line": 12, "col": 5, "severity": "error", "message": "…"}
  ]
}
```

`{"method":"diagnostics"}` returns just the problems list. Both include what
`e` itself found in open files beside the language servers — unknown columns,
missing views and translation keys, PHPStan, and a pending migration's risks
against the data — each with its `source`.

## Driving the editor

| Request | Effect |
| ------- | ------ |
| `{"method":"open","path":"…","line":45,"col":1}` | Open the file and jump to the position |
| `{"method":"focus","target":"terminal\|editor\|agent"}` | Focus a panel |
| `{"method":"notify","message":"…"}` | Post a system notification |
| `{"method":"review_summary","text":"…"}` | Hand the editor a written summary of this session's changes. It becomes the pull-request description when you ship from [session review](source-control.md#session-review-v). |

Example — let the agent jump you to a definition it found:

```sh
printf '{"method":"open","path":"app/Models/User.php","line":58}\n' \
  | nc -U "$E_EDITOR_SOCK"
```

## Language-server co-op

The agent can reuse the editor's already-running language server — no
re-indexing, exact type info:

| Request | Returns |
| ------- | ------- |
| `{"method":"lsp_definition","path":"…","line":12,"col":5}` | `{uri, line, col}` |
| `{"method":"lsp_references","path":"…","line":12,"col":5}` | `{references:[…]}` |
| `{"method":"lsp_hover","path":"…","line":12,"col":5}` | `{hover}` |
| `{"method":"lsp_symbols","query":"User"}` | workspace symbols with locations |
| `{"method":"wait_diagnostics","path":"…","timeout_ms":5000}` | the file's problems once every running server has re-read what you just wrote — `{path, diagnostics:[…], complete}` (`complete:false` on timeout) |

`wait_diagnostics` closes the loop: write the file, ask, fix what comes back.
The editor reloads the buffer from disk (or opens it), waits for each language
server serving the file to publish again, and answers with the merged list; a
file that hasn't changed, or has unsaved edits in the editor, is answered at
once with what is already known.

(The file should be open in the editor so its server is running.)

## Database schema

```sh
printf '{"method":"db_schema"}\n' | nc -U "$E_EDITOR_SOCK"
```

Returns the tables and columns of a connected database (optionally
`"connection":"<name>"`), using the editor's existing, credential-safe
connection — the agent never sees the password.

The agent can also **propose a query**:

```sh
printf '{"method":"db_query","sql":"SELECT count(*) FROM users"}\n' | nc -U "$E_EDITOR_SOCK"
```

The editor shows a consent dialog with the SQL and the target connection. If you
**Allow**, the query runs and the response contains `{columns, rows,
rows_affected, elapsed_ms}`; if you **Deny**, it returns an error. The agent
never gets direct database access.

## The Laravel project in one answer

```sh
printf '{"method":"laravel_map"}\n' | nc -U "$E_EDITOR_SOCK"
```

Everything an agent would otherwise grep its way to, in one reply: the
app's routes (method, URI, action, middleware, and `unprotected` on a write
route without auth), its models with their **live columns** from the
connected database and their relationships, Livewire components with their
public properties and actions, Inertia pages with the controller that
renders each and the props it sends, Blade views and Blade components.

```json
{
  "ok": true,
  "routes": [{"method": "GET|HEAD", "uri": "orders", "name": "orders.index",
              "action": "OrderController@index", "middleware": "web,Authenticate"}],
  "package_routes": 14,
  "models": [{"class": "Order", "table": "orders", "file": "app/Models/Order.php",
              "columns": ["id:bigint unsigned", "paid_at:timestamp?"],
              "relations": ["user: belongsTo User"]}],
  "livewire": [{"name": "order-table", "file": "app/Livewire/OrderTable.php",
                "properties": ["search"], "actions": ["delete"]}],
  "inertia": [{"page": "Orders/Index", "file": "resources/js/Pages/Orders/Index.vue",
               "controller": "app/Http/Controllers/OrderController.php", "props": ["orders"]}],
  "views": ["orders.index"],
  "components": ["alert"]
}
```

Paths are relative to the project root; a column is `name:type`, with `?`
when it's nullable. Routes packages register for themselves (Flux's and
Livewire's assets, say) are only counted, in `package_routes`. Without a
database the models come without columns, and `note` says so.

Two options keep the answer small on a large project:

| Option | Effect |
| ------ | ------ |
| `"include": ["routes", "models"]` | Only these sections (`routes`, `models`, `livewire`, `inertia`, `views`, `components`) |
| `"filter": "order"` | Only entries whose name, URI, action, class or table contains this (any case) |

## Measuring a route

```sh
printf '{"method":"verify_route","uri":"/orders"}\n' | nc -U "$E_EDITOR_SOCK"
```

Replays the route against the running app and answers with its status, time,
query count and any N+1 — the same measurement as the
[verify panel](laravel.md) and the ship gate. The first measurement of a URL
is its **baseline**; each later one comes back compared with it:

```json
{
  "ok": true,
  "url": "https://shop.test/orders",
  "metrics": {"status": 200, "ms": 41, "queries": 2, "slowest_query_ms": 1.2,
              "n_plus_one": []},
  "comparison": {"verdict": "Improved", "summary": "2 queries (-23) · 340ms → 41ms · N+1 removed",
                 "ms_before": 340, "ms_after": 41, "queries_before": 25, "queries_after": 2,
                 "n_plus_one_fixed": true, "n_plus_one_after": false, "status_changed": false}
}
```

So an agent fixing a slow page measures it before it starts and again when it
thinks it's done, and reports a verdict it measured rather than one it
assumed. The verdict is `Improved`, `No change`, `Regressed` or `Broke` (the
route now answers with an error).

- Name the route instead with `"route": "orders.index"`. A route with
  parameters needs them filled in — `"uri": "/orders/1"`.
- Only routes that read are replayed: a POST would change data.
- `"reset": true` makes this measurement the new baseline.
- The editor waits 2.5 s before measuring, so PHP's opcache has seen the file
  the agent just wrote, then takes the median of three requests after a
  warm-up. A time difference under 15% (or 10 ms) counts as noise.
- Query counts come from Clockwork in the app, or from Grove's SQL capture.
  With neither, only status and time are measured, and `note` says so rather
  than reporting zero queries.

Nothing is stashed or checkpointed: the agent's work stays in the tree. The
measurements are recorded in the **Agent Timeline**.

## A test from a captured request

```sh
printf '{"method":"request_test","uri":"/orders/{order}","http_method":"PUT"}\n' | nc -U "$E_EDITOR_SOCK"
```

Writes a feature test from the newest request to that path — or, with a
route's URI and its `{parameters}`, to that route — that Grove captured: the
same test the Runtime panel's 🧪 writes ([below](laravel.md#a-test-from-a-request)).
`"id": "g1254"` picks a request from the Runtime panel instead. The answer is
the test file as it would be with the test added; nothing is written:

```json
{
  "ok": true,
  "file": "/path/to/app/tests/Feature/OrderControllerTest.php",
  "line": 9,
  "test": "PUT /orders/{order}",
  "content": "<?php\n\nuse App\\Models\\Order;\n…",
  "notes": ["Customer has no factory yet: php artisan make:factory CustomerFactory --model=Customer"]
}
```

So an agent fixing a bug a request showed can first write the test that
reproduces it, see it fail, and then make it pass. `notes` says what the test
could not set up by itself.

## Driving the editor

An agent can use the editor's UI the way a person does — run a command, pick a
code action, confirm a dialog, work Cascade, press keys — and read back what
is on screen. That is how `e` is tested end to end:

```sh
q() { printf '%s\n' "$1" | nc -U "$E_EDITOR_SOCK"; }
q '{"method":"open","path":"/app/resources/views/orders/create.blade.php","line":22,"col":31}'
q '{"method":"code_actions"}'                       # the ⌘. menu at the caret
q '{"method":"apply_code_action","title":"Carry"}'  # by index or part of the title
q '{"method":"ui_state"}'                           # the preview it opened
q '{"method":"dialog","action":"confirm"}'
```

| Request | Does |
| ------- | ---- |
| `{"method":"commands"}` | Every named command, `{id, title}` — the palette's list |
| `{"method":"command","id":"session-review"}` | Runs one, as its key binding would |
| `{"method":"code_actions"}` | Opens the `⌘.` menu at the caret; answers with its items once the language servers have answered (`shown: false` and the notification when there are none) |
| `{"method":"apply_code_action","index":0}` | Applies one (`"title":"…"` matches part of a title) |
| `{"method":"dialog","action":"confirm"}` | Confirms (or `cancel`s) the dialog showing — a rename or carry-through preview, or a yes/no question |
| `{"method":"cascade","action":"send","text":"…"}` | Cascade's controls: `send`, `steer`, `next` (queue for after the turn), `stop`, `new_session`, `compact`, `new_worktree`, `back_to_project`, `continue_in`/`take_down` (`"branch"`), `review_turn`/`undo_turn` (`"index"` of the receipt), `answer` (a permission card: `"index"`, `"choice"` by id or name), `agent` (`"id"`: `elyra`, `claude`, `codex`, …, `custom`) |
| `{"method":"wait_idle","timeout_ms":120000}` | Answers once Elyra has finished its turn, with the end of the transcript |
| `{"method":"key","keys":["cmd+p","down","enter"]}` | Presses keys in the editor's window, as the keyboard would: a chord or a list of them (`cmd`, `shift`, `alt`, `ctrl`; named keys such as `enter`, `escape`, `tab`, `up`, `pagedown`, `f12`; other keys by their place on a US keyboard). `"text":"…"` types text. Answers once the keys have been handled. macOS only; needs no Accessibility permission |
| `{"method":"ui_state"}` | What is on screen: open panels, the dialog and what it lists, the code-action menu, the active editor and caret, the session review, Cascade's state and transcript (tool cards, receipts, notices), and the last 20 notifications |

## Running commands

```sh
printf '{"method":"run","command":"php artisan test"}\n' | nc -U "$E_EDITOR_SOCK"
```

Runs a command in the workspace (or `"cwd":"…"`) through the login shell and
returns `{code, stdout, stderr}`. This is the basis for an autonomous
test-fix-rerun loop.

`{"method":"tinker","code":"echo User::count();"}` runs PHP through
`php artisan tinker` and returns `{code, stdout, stderr}` the same way; it is
given up on after 60 seconds.

## Proposing edits (reviewed)

Instead of writing files blindly, an agent can propose a new version and let you
review it hunk-by-hunk:

```sh
printf '{"method":"propose_edit","path":"app/Models/User.php","content":"<full new file>"}\n' \
  | nc -U "$E_EDITOR_SOCK"
```

The editor diffs it against the current file and opens a review overlay where
you **Accept/Reject each hunk**, then **Apply** (or **Cancel**). The reply
reports how many hunks were applied. Accepted changes go into the open buffer
(so they're undoable) or are written to disk.

## Ghost marker & audit timeline

- `{"method":"mark","path":"…","line":42}` (and `open`/`lsp_*`) show where the
  agent is looking as a **🤖 marker** in the status bar — click it to jump there.
- Everything the agent does over the socket is recorded in the **Agent Timeline**
  (⌘⌥A): time, method and a short summary — so you can always answer "what did
  the agent just do?".

## Who can talk to the socket

**The socket path is the capability.** Anything that can reach it can drive the
editor — including `run`, which executes shell commands. Three things guard it:

- The name carries 96 bits of randomness (`agent-<pid>-<random>.sock`), so it
  can't be found by guessing.
- `~/.config/e` is `0700` and the socket itself `0600`, so no other account on
  the machine can list or open it.
- The path is handed out only through `$E_EDITOR_SOCK`, which processes inherit
  from the editor that spawned them.

So an agent you started from `e` can use the socket, and a stray process that
merely knows `~/.config/e` exists cannot.

What this does **not** protect against is code already running as you with
access to that environment variable — a malicious postinstall script in your own
project, say. Treat `$E_EDITOR_SOCK` the way you'd treat a shell: anything
holding it can run commands as you.

`run` and `tinker` deliberately execute without a per-call prompt; the
autonomous test-fix-rerun loop is built on that, and a dialog per command would
break it. `db_query` is the exception — it asks every time, because the risk
there is your data rather than your machine. Everything the agent does is
recorded in the **Agent Timeline** (⌘⌥A).

## Notes

- The socket is local to your machine and per editor process; nothing is exposed
  over the network.
- Available on macOS/Linux (Unix sockets). The path lives under `~/.config/e/`.
- Sockets left behind by editors that are no longer running are cleaned up at
  startup.
