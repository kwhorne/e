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

## Notes

- The socket is local to your machine and per editor process; nothing is exposed
  over the network.
- Available on macOS/Linux (Unix sockets). The path lives under `~/.config/e/`.
