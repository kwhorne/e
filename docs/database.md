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

Results are capped at 1000 rows.

## Editing data

In the **Data** view of a table that has a primary key:

- **Double-click a cell** to open the edit dialog. Change the value (or tick
  **NULL**) and press **⌘↵** to save.
- The dialog also offers:
  - **Delete row** — removes the row (requires a primary key).
  - **Follow FK →** — if the column is a foreign key, jumps to the referenced
    table filtered to the linked value.
  - **Filter to value** — restricts the current table to rows matching that
    cell (`WHERE col = value` / `IS NULL`), composing with sort and pagination.
    An active filter appears as a chip in the toolbar — click it to clear.
- **+ Row** on the toolbar opens an insert dialog with one field per column
  (each with a NULL toggle). Columns left blank (and not marked NULL) are
  omitted, so database defaults and auto-increment apply.

### Write protection

Connections that look like **production** — a remote (non-localhost) host, an
SSH tunnel, or a name containing `prod`/`production`/`live` — default to
**read-only**, and edits/inserts/deletes are refused. A 🔒 / 🔓 badge on the
connection shows the state; click it to toggle writes on or off.

## Inline SQL in PHP

SQL inside PHP strings (`DB::select(...)`, `->whereRaw(...)`, and similar) is
syntax-highlighted. With the cursor inside such a string and a database
connected:

- **⌘↵** — **Run SQL under cursor**: runs the query and shows the result grid.
- **⌥⌘↵** — **Explain SQL under cursor**: runs the engine's `EXPLAIN` and flags
  full table scans / missing indexes.
- **Suggest Index for SQL under cursor** (command palette) — runs `EXPLAIN`,
  and if it finds a problem, asks the AI agent to propose a Laravel migration
  that adds the missing index.

Schema-aware completion suggests table and column names as you type SQL.

## Connection actions

Hover a connection for its actions: **⌗** new query, **⟳** refresh tables,
**⏏** disconnect, **✎** edit, **✕** remove.
