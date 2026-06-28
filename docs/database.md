# Database

`e` includes a Database panel for browsing and querying your project's
databases without leaving the editor. It supports **MySQL/MariaDB**,
**PostgreSQL** and **SQLite**.

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

Connections are saved per project in `~/.config/e/databases.json` — never written
into the project folder, so nothing can be committed.

## Browsing & querying

- Click a connection to **connect** and list its tables.
- Use the **filter** box to narrow the table list.
- Click a table to open its rows in the results grid (`SELECT * … LIMIT 200`).
- Click **⌗** on a connection to open a blank **query editor**. Type SQL and
  press **⌘↵** (or **Run**) to execute. `SELECT`/`SHOW`/`EXPLAIN` show a grid;
  other statements report the number of affected rows.

Row and `NULL` values are shown in the grid; results are capped at 1000 rows.

## Connection actions

Hover a connection for its actions: **⌗** new query, **⟳** refresh tables,
**⏏** disconnect, **✕** remove.
