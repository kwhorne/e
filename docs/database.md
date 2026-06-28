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
  - Switch between **Data** and **Structure** (columns, types, nullability,
    keys; primary keys are marked 🔑).
  - Click a **column header** to sort (ascending → descending → off).
  - Page through rows with **‹ Prev** / **Next ›**, or pan with the arrow keys.
  - **Double-click a cell** to edit it (in a table with a primary key); ⌘↵ saves.
  - **💾** saves the current SQL as a named query; **Saved ▾** loads it back.
  - **⬇ CSV** exports the current result to a file.
- Click **⌗** on a connection to open a blank **query editor**. Type SQL and
  press **⌘↵** (or **Run**) to execute. `SELECT`/`SHOW`/`EXPLAIN` show a grid;
  other statements report the number of affected rows.

Results are capped at 1000 rows.

## Connection actions

Hover a connection for its actions: **⌗** new query, **⟳** refresh tables,
**⏏** disconnect, **✎** edit, **✕** remove.
