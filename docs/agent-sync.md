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

`{"method":"diagnostics"}` returns just the problems list.

## Driving the editor

| Request | Effect |
| ------- | ------ |
| `{"method":"open","path":"…","line":45,"col":1}` | Open the file and jump to the position |
| `{"method":"focus","target":"terminal\|editor\|agent"}` | Focus a panel |
| `{"method":"notify","message":"…"}` | Post a system notification |

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

## Running commands

```sh
printf '{"method":"run","command":"php artisan test"}\n' | nc -U "$E_EDITOR_SOCK"
```

Runs a command in the workspace (or `"cwd":"…"`) through the login shell and
returns `{code, stdout, stderr}`. This is the basis for an autonomous
test-fix-rerun loop.

## Notes

- The socket is local to your machine and per editor process; nothing is exposed
  over the network.
- Available on macOS/Linux (Unix sockets). The path lives under `~/.config/e/`.
