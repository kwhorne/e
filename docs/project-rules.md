# Project Rules

Every team has rules that no linter knows: *models list their `$fillable`*,
*every Billing controller action checks authorization*, *money is integer
cents*. Write them down once in **`.e/rules.toml`** at the project root, and
`e` holds every change to them:

- **As you type.** A file that breaks a rule is underlined where it breaks it,
  with what the rule says (source `rules`).
- **In Session Review and at the ship gate.** A change that breaks a rule
  where it changed gets a `project-rule` flag. An `error` rule blocks the
  ship gate.
- **For agents.** A new Cascade session's first message starts with the
  rules. Every agent can read them over the socket, and `laravel_map` carries
  them.

**Project Rules: Open** (command palette) opens the file. If there isn't one
yet, it starts it with the shape of a rule. **Project Rules: Check Project**
lists every place the project breaks a rule now, in the picker, so you can
see what a new rule would catch before you commit it.

## Writing a rule

```toml
[[rule]]
id = "no-unguarded-models"
says = "Models list their $fillable; never $guarded = []"
files = ["app/Models"]
forbid = '\$guarded\s*=\s*\[\s*\]'

[[rule]]
id = "billing-authorizes"
says = "Every Billing controller action checks authorization"
files = ["app/Http/Controllers/Billing"]
require = '\$this->authorize\(|Gate::'
per = "method"
level = "error"

[[rule]]
says = "Money is stored as integer cents, never a float"
```

| Key | Meaning |
| --- | ------- |
| `says` | What the rule says, as you'd tell a colleague. This is the message, and what agents are told. Required. |
| `id` | A short name, shown with the message. Defaults to `rule-N`. |
| `files` | Globs from the project root the rule covers. A directory covers what's in it, and a bare name matches that file anywhere. `*` stops at `/` and `**` doesn't. Leave it out to cover every file. |
| `except` | Globs the rule doesn't cover. |
| `forbid` | A pattern ([regex](https://docs.rs/regex/latest/regex/#syntax)) no covered file may contain. Lines that are comments don't count. |
| `require` | A pattern each covered file must contain. |
| `per` | `"method"`: `require` applies to each public method instead of the whole file. Constructors and other magic methods are left out, but `__invoke` is not. |
| `level` | `"warning"` (the default) or `"error"`. |

Use single quotes for patterns: TOML leaves `'…'` alone, so `\$` and `\(`
reach the pattern as written. `vendor/`, `node_modules/` and `storage/` are
never checked.

A rule with neither `forbid` nor `require` isn't checked. Agents are still
told it, which is the place for a rule no pattern can capture.

If the file doesn't read (a typo, an unknown key, a pattern that isn't one),
the line is underlined in the editor and `e` keeps the rules it last read.
The rules are read again when you save the file and after a checkout, pull
or merge.

## In Session Review

The review is scoped to the diff, like the other [automated
flags](source-control.md#automated-flags):

- A `forbid` match is flagged only on a line the session added.
- A method missing what `require` asks for is flagged only if the session
  changed the method.
- A file missing it is flagged when the session changed the file.

So an old violation nobody touched doesn't block today's change. **Check
Project** finds those.

## For agents

```sh
printf '{"method":"rules","check":true}\n' | nc -U "$E_EDITOR_SOCK"
```

```json
{
  "ok": true,
  "file": ".e/rules.toml",
  "rules": [{"id": "billing-authorizes", "says": "Every Billing controller action checks authorization",
             "files": ["app/Http/Controllers/Billing"], "level": "error", "checked": true}],
  "breaches": [{"path": "app/Http/Controllers/Billing/InvoiceController.php",
                "line": 13, "col": 21, "rule": "billing-authorizes",
                "says": "Every Billing controller action checks authorization"}]
}
```

Without `check`, the reply has only the rules. If the file doesn't read,
`error` says where.
