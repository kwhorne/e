# Database

`e` includes a Database panel for browsing and querying your project's
databases without leaving the editor. It supports **MySQL/MariaDB**,
**PostgreSQL**, **SQLite** and **ClickHouse** (over its HTTP interface), with an
optional **SSH tunnel** for remote databases.

Toggle it with **⌘3** (or the command palette → *Toggle Database Panel*). The
panel sits on the **right** by default; move it to the left under
**Settings → Database panel**.

## Adding a connection

Click **＋** in the panel header:

- **From .env** — reads `DB_CONNECTION`, `DB_HOST`, `DB_DATABASE`, `DB_USERNAME`,
  `DB_PASSWORD` (and SQLite's `DB_DATABASE` path) from the project's `.env`,
  following Laravel conventions.
- **Manually** — pick an engine and fill in host / port / database / user /
  password, or a file path for SQLite.

For remote databases, tick **Use SSH tunnel** and fill in the SSH host, user and
either a private key or a password — `e` forwards a local port through the system
`ssh` for the lifetime of the connection.

Connections are saved per project in `~/.config/e/databases.json` — never written
into the project folder, so nothing can be committed.

### TLS

The **TLS** row sets how MySQL, PostgreSQL and ClickHouse connections are
encrypted:

| Setting | What happens |
| --- | --- |
| **Auto** (default) | A direct connection to a host that isn't this machine is encrypted when the server offers TLS, without checking its certificate (PostgreSQL's `prefer`). Local hosts and SSH tunnels go without, since a tunnel is already encrypted. ClickHouse serves HTTP and HTTPS on different ports, so auto uses HTTPS on 8443 and 443 and HTTP otherwise. |
| **Off** | Never encrypted. |
| **Require** | Always encrypted; the server's certificate isn't checked. |
| **Verify** | Always encrypted; the certificate must chain to a CA the system trusts, or to the **CA certificate** file you give (for RDS, Cloud SQL or your own CA), and name the host. |

Through an SSH tunnel, **Verify** checks the certificate against the database
host's real name, not `127.0.0.1`. MySQL is the exception: its driver can't be
told the name, so there only the chain is checked.

**From .env** carries TLS over: PostgreSQL's `DB_SSLMODE` (`disable`, `require`,
`verify-ca`/`verify-full` → Verify, anything else → Auto), and MySQL's
`MYSQL_ATTR_SSL_CA`, which turns on Verify with that CA.

### Where passwords are kept

Database passwords, SSH passwords and SSH key passphrases are **not** stored in
`databases.json`. They go to the operating system's credential store:

| Platform | Store |
| --- | --- |
| macOS | Keychain |
| Linux | Secret Service (GNOME Keyring, KWallet, …) |

`databases.json` holds only the non-secret fields plus a reference, and is
written `0600` (readable by your user only). If no credential store is reachable
— a headless Linux box with no Secret Service, say — `e` falls back to keeping
the secrets in that file, still `0600`, and says so on stderr.

Connections saved by an earlier version keep working: their secrets move to the
credential store the next time the connection list is saved, and the file's
permissions are tightened as soon as `e` reads it.

When adding manually you can press **Test** to verify the connection before
saving.

## Browsing & querying

- Click a connection to **connect** and list its tables.
- Use the **filter** box to narrow the table list.
- Click a table to open its rows in the results grid (200 per page).
  - Switch between **Data** and **Structure**. Structure lists columns (types,
    nullability, keys — primary keys are marked 🔑) and the table's **indexes**
    (name, `UNIQUE`/`INDEX`, and the columns each covers).
  - Click a **column header** to sort (ascending → descending → off).
  - Page through rows with **‹ Prev** / **Next ›**, or pan with the arrow keys.
  - **💾** saves the current SQL as a named query; **Saved ▾** loads it back.
  - **⬇ CSV** exports the current result to a file.
- Click **⌗** on a connection to open a blank **query editor**. Type SQL and
  press **⌘↵** (or **Run**) to execute. `SELECT`/`SHOW`/`EXPLAIN` show a grid;
  other statements report the number of affected rows.

A table is read a page at a time, so it has no row limit. A query in the
console shows at most 1000 rows; the result says so when it was cut there.

## Editing data

In the **Data** view of a table that has a primary key:

- **Double-click a cell** to open the edit dialog. The value opens in a
  multi-line editor, for long text and JSON alike; change it (or tick
  **NULL**) and press **⌘↵**, or **Esc** to cancel. A `json`/`jsonb` column
  only takes valid JSON: the dialog says what's wrong and stays open. A JSON
  value on one line opens laid out over several, keys in their own order and
  numbers as written; saved unchanged it stays exactly as it was, and edited
  it goes back on one line, spaced the way it was read. The value viewer
  lays JSON out the same way. Edits
  are **staged**, not written immediately — see
  [Environments & safety](#environments--safety) for Submit/Revert.
- The dialog also offers:
  - **Delete row** — stages a deletion (requires a primary key).
  - **Follow FK →** — if the column is a foreign key, jumps to the referenced
    table filtered to the linked value.
  - **Related →** — shows rows in other tables that reference this row (reverse
    foreign keys).
  - **Filter to value** — restricts the current table to rows matching that
    cell (`WHERE col = value` / `IS NULL`), composing with sort and pagination.
    An active filter appears as a chip in the toolbar — click it to clear.
- **+ Row** on the toolbar opens a dialog with one field per column (each
  with a NULL toggle). Columns left blank (and not marked NULL) are omitted,
  so database defaults and auto-increment apply. **Add row** stages it like
  an edit: it shows in green above the table's rows (a left-out column reads
  *default*) and is inserted on **Submit** with the other changes. Every
  insert is in the **Log**, with an **Undo** when the dialog gave the
  primary key.

A connection that looks like **production** defaults to **read-only** (a
🔒 / 🔓 badge toggles it); see below.

## Inline SQL in PHP

SQL inside PHP strings (`DB::select(...)`, `->whereRaw(...)`, and similar) is
syntax-highlighted. With the cursor inside such a string and a database
connected:

- **⌘↵** — **Run SQL under cursor**: runs the query and shows the result grid.
  Outside a SQL string in a Laravel project, `⌘↵` instead
  [evaluates the PHP expression](laravel.md#evaluate-an-expression-where-it-is-written)
  at the caret.
- **⌥⌘↵** — **Explain SQL under cursor**: runs the engine's `EXPLAIN`, shows
  the plan in an **EXPLAIN** result tab and flags full table scans / missing
  indexes; **Suggest Index → ask the agent** on that banner asks the AI agent
  for a migration that adds the index. On an Eloquent chain outside a SQL
  string, it [explains the queries the chain runs](laravel.md#explain-the-query)
  and offers the missing index as a migration.
- **Suggest Index for SQL under cursor** (command palette) — runs `EXPLAIN`,
  and if it finds a problem, asks the AI agent to propose a Laravel migration
  that adds the missing index.

Both run against the panel's **active connection** (see
[below](#the-active-connection)), or, if none is chosen and only one is
connected, that one. With several connected and none chosen, `e` asks you to
pick one rather than guess.

Schema-aware completion suggests table and column names as you type SQL,
from the project's `.env` database.

## The SQL console

The query box in the results panel is a full SQL editor: syntax highlighting,
schema-aware completion as you type, and a draggable handle below it to
resize. Completion offers:

- **tables** after `FROM`, `JOIN`, `INTO`, `UPDATE` and `TABLE`, and after a
  comma in a `FROM` list;
- after **`alias.`** or **`table.`**, the columns of that table. The alias
  can be defined anywhere in the statement, so `SELECT u.| FROM users u`
  works;
- elsewhere, the **columns of the tables the statement names** (every
  table's while it names none), and **SQL keywords** — those that fit the
  clause you're in first (`AND`, `IS NULL`, `LIKE` in a `WHERE`; `ASC`,
  `DESC` after `ORDER BY`), in the case you type.

Nothing is offered inside a string or a comment. Names come from the
database the console runs against, read the first time you type and again
after **⟳** or a reconnect. SQL inside PHP strings completes the same way.

- **⌘↵** runs the **selection**, or the **statement under the cursor**;
  **⌘⇧↵** (or **Run**) runs the whole console. Each statement gets its own
  **result tab** — pin one (★) to keep it across runs. Hover a tab to see
  the statement it came from.
- Result tabs stay when you open a table. The table has a tab of its own
  (**▦ name**) at the start of the strip: click it to go back to the table at
  the page, sort and filter you left it. Rows in a result tab are read-only;
  only the table's tab edits.
- **`:param`** placeholders prompt for values before running (remembered).
- **○ History** opens a searchable log of every query you've run in this project;
  click one to load it back into the console.
- Long queries show a **✕ Cancel** while running. In a run of several
  statements, Cancel stops the one running and runs none of the rest.
- Export the result as **CSV / JSON / SQL**, or copy it as **TSV / Markdown**.

## Environments & safety

Each connection is labelled **local**, **staging** or **production** (green /
amber / red), shown as a dot + badge in the list and on the active connection.
The label is guessed from the host and names: a remote host is staging, and
`prod` in its label, database or host makes it production.

When the guess is wrong, set it under **Environment** in the connection form
(**✎**): **Auto** shows the guess, or pick **local**, **staging** or
**production**. A connection set to production starts read-only; one set to
local runs writes without asking, and the form warns when that overrides a
guess of staging or production.

The label drives the safety rails:

- **Destructive statements** (`DROP`, `TRUNCATE`, `DELETE`/`UPDATE` without a
  `WHERE`) and **any write to a non-local database** open a confirmation dialog
  that lists the exact statements and **how many rows each touches**;
  non-local writes require an explicit acknowledgement. The rows are counted
  on the database while the dialog is open, by running the statement's
  `WHERE` as a `SELECT` (stopping at 10,000, shown as `10,000+`); an
  `INSERT … VALUES` counts its own rows. Statements it can't read safely —
  joins, `UPDATE … FROM`, `DELETE … USING`, a `WHERE` that calls a function
  that writes — say *rows not counted* rather than guess.
- **Transactional editing:** cell edits, new rows and row deletes are
  *staged* (edited cells turn amber, new rows green, deleted rows red and
  struck through). They stay staged while you page, sort,
  filter, open another table or run a query, until you submit or revert them.
  A **N pending changes to `table`** bar offers **Submit** (all in one
  transaction, via the confirmation dialog) and **Revert**. Staged changes
  belong to one table at a time: to edit another, submit or revert the first.
  A **Log** button shows the session's writes with an **Undo** where a reverse
  statement can be generated.
- **Conflicts:** Submit first reads each edited and deleted row again, in the
  same transaction, and checks that the edited cell still holds the value you
  read (compared as the grid showed it), and that a row to delete is still
  there. If someone changed it meanwhile, **nothing is written**: a red banner
  names each such row and what it holds now, those changes are no longer
  staged, the table is read again, and the rest stay staged for another
  Submit. The rows are locked while they are checked (`FOR UPDATE`; SQLite
  takes its write lock first), so nothing gets in between. ClickHouse has no
  transactions, so there the check runs just before the writes.
- **Snapshots:** the **⤓** action on a local connection dumps the database
  (SQLite copy / mysqldump / pg_dump) into `~/.config/e/snapshots/` — handy
  before a migration.

## Structure, relationships & tooling

- The **Structure** tab lists columns and indexes; **Copy DDL** copies the
  `CREATE TABLE`, and **+ Migration** scaffolds a Laravel migration for the table
  and opens it (instead of running DDL directly).
- The **⇄** button in the panel header shows **schema relationships** (every
  foreign key as `table.column → ref_table.ref_column`) of the active
  connection.
- The cell dialog can **Follow FK →** (jump to the referenced row) and show
  **Related →** rows (reverse foreign keys).
- **Seed 10** (local tables) creates rows via the Eloquent factory through
  Tinker; **⬆ CSV** imports a CSV into the table.
- **Search all data…** in the panel header scans every table's text columns of
  the active connection for a value, and shows one result tab per
  matching table. **✕ Cancel** stops it
  before the next table.
- Row counts appear next to each table in the tree. They come from the
  server's catalog in one query rather than a count of every table:
  - **PostgreSQL** shows the planner's estimate, as `≈12.3k`. A table is
    blank until it has been analyzed.
  - **MySQL** shows the estimate from `TABLE_ROWS`, also with `≈`.
  - **ClickHouse** shows its exact counts.
  - **SQLite** counts each table.

  Opening a table counts its rows exactly.

## The active connection

The chip under the panel's title, and at the top of the results panel, shows
the **active connection** in its environment's colour (`● app · production
▾`). The console runs against it, and **Search all data**, **⇄**
relationships, EXPLAIN from the runtime panel and SQL under the cursor use it.

Click the chip to switch: the menu lists every connection with its
environment, and picking one that isn't connected connects it. A table open
on the previous connection is left — its tab brings it back — and result tabs
stay. Opening a table, or **⌗** on a connection, makes that connection active
too, and the first one you connect is active until you choose another.

## Connection actions

Hover a connection for its actions: **⌗** new query, **⟳** refresh tables,
**⤓** snapshot, **⏏** disconnect, **✎** edit, **✕** remove.
