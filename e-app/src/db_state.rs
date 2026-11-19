//! Database panel state: connections, table browsing, querying and inline cell
//! editing across MySQL, PostgreSQL, SQLite and ClickHouse.
//!
//! The view lives in [`crate::db_view`]; this module owns the `AppState` methods
//! that drive it. Extracted from the former `state.rs` god-module (fields stay on
//! `AppState`); same pattern as [`crate::debug`] / [`crate::runtime`].

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use floem::ext_event::create_ext_action;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::selection::Selection;
use floem::views::editor::text::Document;

use e_core::language::Language;

use crate::state::{AppState, DbEntry, DbForm, InsertField, RowPk};

/// Rows per page when browsing a table in the Database panel.
pub(crate) const DB_PAGE: usize = 200;

/// Result of connecting: the live connection plus its table and view names.
type ConnectResult = Result<(Arc<e_db::Conn>, Vec<String>, Vec<String>), String>;

impl AppState {
    // ---- Database panel ------------------------------------------------

    pub fn toggle_db_panel(&self) {
        let open = !self.db.open.get_untracked();
        self.db.open.set(open);
        if open && self.db.conns.with_untracked(|c| c.is_empty()) {
            self.load_databases();
        }
    }

    /// Load saved connections for the project; offer `.env` detection if empty.
    pub fn load_databases(&self) {
        let root = self.root.get_untracked();
        let saved = e_db::load_connections(&root);
        let entries: Vec<DbEntry> = saved
            .into_iter()
            .map(|c| DbEntry::new(self.cx, c))
            .collect();
        self.db.conns.set(entries);
        self.db.queries.set(e_db::load_queries(&root));
    }

    /// Fetch the project's DB schema (from `.env`) into an in-memory cache for
    /// Eloquent attribute completion. Runs in the background.
    pub fn load_db_schema_cache(&self) {
        let root = self.root.get_untracked();
        let Some(cfg) = e_db::from_env(&root) else {
            return;
        };
        let state = *self;
        let send = create_ext_action(
            self.cx,
            move |m: std::collections::HashMap<String, Vec<e_db::ColumnInfo>>| {
                state.db.schema_cache.set(std::sync::Arc::new(m));
                // A project that keeps the Eloquent helper gets it refreshed
                // from the schema it was just handed.
                state.maybe_regenerate_eloquent_helper();
                // The column lint ran against the old schema: a column just
                // migrated in is no longer unknown.
                let php: Vec<u64> = state.editor.buffers.with_untracked(|bs| {
                    bs.iter()
                        .filter(|b| b.file.language == e_core::language::Language::Php)
                        .map(|b| b.id)
                        .collect()
                });
                for id in php {
                    state.refresh_lint(id);
                }
            },
        );
        std::thread::spawn(move || {
            let mut map = std::collections::HashMap::new();
            if let Ok(conn) = e_db::connect(&cfg) {
                if let Ok(tables) = e_db::tables(&conn) {
                    for t in tables {
                        if let Ok(cols) = e_db::columns(&conn, &t) {
                            map.insert(t, cols);
                        }
                    }
                }
            }
            send(map);
        });
    }

    /// Re-read the schema cache once the project's database has been
    /// migrated — however that happened: the Migrations panel, a terminal,
    /// Grove. Called from the idle tick and acts every ~10 s; a migration shows
    /// up as a change in Laravel's `migrations` table. Only a local database
    /// is polled: a remote one isn't queried behind the user's back.
    pub fn check_db_schema_freshness(&self) {
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
        static TICK: AtomicU64 = AtomicU64::new(0);
        static BUSY: AtomicBool = AtomicBool::new(false);
        static LAST: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
        if TICK.fetch_add(1, Ordering::Relaxed) % 20 != 19 || BUSY.load(Ordering::Relaxed) {
            return;
        }
        let root = self.root.get_untracked();
        let Some(cfg) = e_db::from_env(&root).filter(|c| c.environment().is_local()) else {
            return;
        };
        BUSY.store(true, Ordering::Relaxed);
        let state = *self;
        self.spawn_bg(
            move || {
                let conn = e_db::connect(&cfg).ok()?;
                let r = e_db::query(
                    &conn,
                    "SELECT COUNT(*), MAX(id), MAX(batch) FROM migrations",
                    1,
                )
                .ok()?;
                Some(format!("{:?}", r.rows))
            },
            move |fp: Option<String>| {
                BUSY.store(false, Ordering::Relaxed);
                let Some(fp) = fp else {
                    return;
                };
                let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
                let changed = last.as_ref().is_some_and(|prev| *prev != fp);
                *last = Some(fp);
                drop(last);
                if changed {
                    eprintln!("e: the database was migrated — reloading its schema");
                    state.load_db_schema_cache();
                }
            },
        );
    }

    /// Save the current query editor text under the typed name.
    pub fn db_save_query(&self) {
        let name = self.db.query_name.get_untracked().trim().to_string();
        let sql = self.db.query_text.get_untracked();
        if name.is_empty() || sql.trim().is_empty() {
            self.db.saving_query.set(false);
            return;
        }
        self.db.queries.update(|q| {
            q.retain(|x| x.name != name);
            q.push(e_db::SavedQuery { name, sql });
            q.sort_by(|a, b| a.name.cmp(&b.name));
        });
        let _ = e_db::save_queries(&self.root.get_untracked(), &self.db.queries.get_untracked());
        self.db.query_name.set(String::new());
        self.db.saving_query.set(false);
    }

    /// Load a saved query into the editor.
    pub fn db_load_query(&self, sql: String) {
        self.set_console_sql(sql);
    }

    /// Open the query-history panel and load the most recent entries.
    pub fn db_open_history(&self) {
        self.db.history_query.set(String::new());
        self.db_reload_history();
        self.db.history_open.set(true);
    }

    /// (Re)load history entries for the current project, honouring the search box.
    pub fn db_reload_history(&self) {
        let Some(path) = crate::config::history_db_path() else {
            return;
        };
        let project = self.root.get_untracked().to_string_lossy().into_owned();
        let needle = self.db.history_query.get_untracked();
        let entries = if needle.trim().is_empty() {
            e_db::history::recent(&path, &project, 200)
        } else {
            e_db::history::search(&path, &project, needle.trim(), 200)
        }
        .unwrap_or_default();
        self.db.history.set(entries);
    }

    /// Open the session write-log (undo-log) panel.
    pub fn db_open_write_log(&self) {
        self.db.write_log_open.set(true);
    }

    /// Run a logged reverse statement to undo a change (gated like any run).
    /// Undo a logged write: its reverse statement, on the connection it was
    /// written on — not whichever is active now.
    pub fn db_undo_write(&self, key: String, reverse_sql: String) {
        self.db.write_log_open.set(false);
        let Some(entry) = self
            .db
            .conns
            .with_untracked(|c| c.iter().find(|e| e.key() == key).cloned())
        else {
            Self::notify("Undo: the connection it was written on is gone");
            return;
        };
        if self.db.result_key.get_untracked().as_deref() != Some(key.as_str()) {
            self.db_set_active(entry);
        }
        self.run_console_sql(reverse_sql);
    }

    /// Load a history entry's SQL back into the console and close the panel.
    pub fn db_reopen_history(&self, sql: String) {
        self.set_console_sql(sql);
        self.db.history_open.set(false);
    }

    /// Clear all query history for the current project.
    pub fn db_clear_history(&self) {
        if let Some(path) = crate::config::history_db_path() {
            let project = self.root.get_untracked().to_string_lossy().into_owned();
            let _ = e_db::history::clear(&path, &project);
        }
        self.db.history.set(Vec::new());
    }

    /// Set the SQL console text, keeping the `db_query_text` signal and the
    /// editor's document in sync. Programmatic callers (browse queries,
    /// run-under-cursor, saved/history queries) go through here so the editor
    /// reflects the change; the editor's own edits mirror back the other way.
    pub fn set_console_sql(&self, sql: String) {
        self.db.query_text.set(sql.clone());
        if let Some(doc) = self.db.console_doc.get_untracked() {
            if doc.text().to_string() != sql {
                let len = doc.text().len();
                doc.edit_single(Selection::region(0, len), &sql, EditType::InsertChars);
            }
        }
    }

    #[allow(dead_code)]
    pub fn db_delete_query(&self, name: String) {
        self.db.queries.update(|q| q.retain(|x| x.name != name));
        let _ = e_db::save_queries(&self.root.get_untracked(), &self.db.queries.get_untracked());
    }

    fn db_persist(&self) {
        let root = self.root.get_untracked();
        let configs: Vec<e_db::DbConfig> = self
            .db
            .conns
            .with_untracked(|c| c.iter().map(|e| e.config.clone()).collect());
        let _ = e_db::save_connections(&root, &configs);
    }

    /// Add the connection inferred from the project's `.env`.
    pub fn db_add_from_env(&self) {
        let root = self.root.get_untracked();
        let Some(cfg) = e_db::from_env(&root) else {
            Self::notify("No DB_CONNECTION found in .env");
            return;
        };
        self.db_add_config(cfg);
    }

    fn db_add_config(&self, cfg: e_db::DbConfig) {
        let key = cfg.key();
        if self
            .db
            .conns
            .with_untracked(|c| c.iter().any(|e| e.key() == key))
        {
            return;
        }
        let entry = DbEntry::new(self.cx, cfg);
        self.db.conns.update(|c| c.push(entry.clone()));
        self.db_persist();
        self.db_connect(entry);
    }

    pub fn db_remove(&self, key: String) {
        self.db_forget_browsed(&key);
        self.db.conns.update(|c| c.retain(|e| e.key() != key));
        self.db_persist();
    }

    pub fn db_connect(&self, entry: DbEntry) {
        if entry.connecting.get_untracked() {
            return;
        }
        entry.connecting.set(true);
        entry.error.set(None);
        let cfg = entry.config.clone();
        let state = *self;
        let send = create_ext_action(self.cx, move |res: ConnectResult| {
            entry.connecting.set(false);
            match res {
                Ok((conn, tables, views)) => {
                    // e-db enforces the panel's read-only toggle itself, so the
                    // console, CSV import and agent queries all honour it.
                    conn.set_read_only(entry.read_only.get_untracked());
                    entry.schema.set(None);
                    entry.conn.set(Some(conn));
                    // The first one connected is the active one until
                    // another is chosen.
                    if state.db.result_key.with_untracked(|k| k.is_none()) {
                        state.db.result_key.set(Some(entry.key()));
                        state
                            .db
                            .result_title
                            .set(format!("{} · query", entry.config.display_name()));
                    }
                    entry.tables.set(tables);
                    entry.views.set(views);
                    entry.expanded.set(true);
                    state.db_load_table_counts(entry.clone());
                }
                Err(e) => entry.error.set(Some(e)),
            }
        });
        std::thread::spawn(move || {
            let res = e_db::connect(&cfg).and_then(|conn| {
                let conn = Arc::new(conn);
                let tables = e_db::tables(&conn)?;
                let views = e_db::views(&conn).unwrap_or_default();
                Ok((conn, tables, views))
            });
            send(res);
        });
    }

    pub fn db_disconnect(&self, entry: DbEntry) {
        self.db_forget_browsed(&entry.key());
        entry.conn.set(None);
        entry.tables.set(Vec::new());
        entry.schema.set(None);
        entry.expanded.set(false);
    }

    /// Drop the browsed table's tab when its connection goes away.
    fn db_forget_browsed(&self, key: &str) {
        if self
            .db
            .browsed
            .with_untracked(|b| b.as_ref().is_some_and(|(k, _)| k == key))
        {
            self.db.browsed.set(None);
        }
    }

    /// The schema SQL console completion offers: that of the connection the
    /// console runs against, loaded on first use (`None` until it is), or
    /// the project's `.env` database while the console has none.
    pub(crate) fn db_console_schema(&self) -> Option<Arc<crate::state::TableColumns>> {
        let Some(key) = self.db.result_key.get_untracked() else {
            return Some(self.db.schema_cache.get_untracked());
        };
        let entry = self
            .db
            .conns
            .with_untracked(|c| c.iter().find(|e| e.key() == key).cloned())?;
        if let Some(schema) = entry.schema.get_untracked() {
            return Some(schema);
        }
        self.db_load_conn_schema(entry);
        None
    }

    /// Read every table's and view's columns on `entry`'s connection into
    /// its schema, in the background.
    fn db_load_conn_schema(&self, entry: DbEntry) {
        if entry.schema_loading.get_untracked() {
            return;
        }
        let Some(conn) = entry.conn.get_untracked() else {
            return;
        };
        entry.schema_loading.set(true);
        let mut names = entry.tables.get_untracked();
        names.extend(entry.views.get_untracked());
        let (schema, loading, live) = (entry.schema, entry.schema_loading, entry.conn);
        let send = create_ext_action(self.cx, {
            let conn = conn.clone();
            move |map: crate::state::TableColumns| {
                loading.set(false);
                // Not if the connection was closed or remade meanwhile.
                if live.with_untracked(|c| c.as_ref().is_some_and(|c| Arc::ptr_eq(c, &conn))) {
                    schema.set(Some(Arc::new(map)));
                }
            }
        });
        std::thread::spawn(move || {
            let map = names
                .into_iter()
                .filter_map(|t| e_db::columns(&conn, &t).ok().map(|cols| (t, cols)))
                .collect();
            send(map);
        });
    }

    pub fn db_toggle(&self, entry: DbEntry) {
        if entry.conn.get_untracked().is_some() {
            let expanding = !entry.expanded.get_untracked();
            entry.expanded.set(expanding);
            if expanding {
                self.db_load_table_counts(entry);
            }
        } else {
            self.db_connect(entry);
        }
    }

    /// Lazily load approximate row counts for every table (shown in the tree).
    /// Loaded once per connection; runs off the UI thread.
    fn db_load_table_counts(&self, entry: DbEntry) {
        if !entry.table_counts.with_untracked(|m| m.is_empty()) {
            return; // already loaded
        }
        let Some(conn) = entry.conn.get_untracked() else {
            return;
        };
        let tables = entry.tables.get_untracked();
        if tables.is_empty() {
            return;
        }
        let counts_sig = entry.table_counts;
        let send = create_ext_action(
            self.cx,
            move |counts: std::collections::HashMap<String, e_db::RowCount>| {
                counts_sig.set(counts);
            },
        );
        // The server's own figures in one query, not a COUNT(*) per table.
        std::thread::spawn(move || {
            send(e_db::row_counts(&conn, &tables).unwrap_or_default());
        });
    }

    pub fn db_refresh_tables(&self, entry: DbEntry) {
        let Some(conn) = entry.conn.get_untracked() else {
            return;
        };
        let send = create_ext_action(self.cx, move |tables: Vec<String>| {
            entry.tables.set(tables);
            // Read again on next use, with the tables as they are now.
            entry.schema.set(None);
        });
        std::thread::spawn(move || {
            let tables = e_db::tables(&conn).unwrap_or_default();
            send(tables);
        });
    }

    /// Open a table's rows in the results overlay.
    pub fn db_open_table(&self, entry: DbEntry, table: String) {
        if entry.conn.get_untracked().is_none() {
            return;
        }
        self.db.result_key.set(Some(entry.key()));
        self.db.result_table.set(Some(table.clone()));
        self.db
            .result_title
            .set(format!("{} · {}", entry.config.display_name(), table));
        self.db.subview.set("data".into());
        self.db.sort.set(None);
        self.db.filter.set(None);
        self.db.page.set(0);
        self.db.total_rows.set(None);
        self.db.columns.set(Vec::new());
        self.db.ddl.set(None);
        self.db.result_open.set(true);
        self.db_load_columns(entry.clone(), table.clone());
        self.db_reload_table();
    }

    /// (Re)run the browse query for the current table, sort and page.
    pub fn db_reload_table(&self) {
        let (Some(key), Some(table)) = (
            self.db.result_key.get_untracked(),
            self.db.result_table.get_untracked(),
        ) else {
            return;
        };
        let Some(entry) = self
            .db
            .conns
            .with_untracked(|c| c.iter().find(|e| e.key() == key).cloned())
        else {
            return;
        };
        let Some(conn) = entry.conn.get_untracked() else {
            return;
        };
        // Leaving a result tab for the table: its rows mustn't stay in the
        // grid, where they'd pass for the table's, while the page loads.
        if self.db.active_tab.get_untracked().is_some() {
            self.db.active_tab.set(None);
            self.db.result.set(None);
            self.db.explain_issues.set(Vec::new());
        }
        let engine = entry.config.engine.clone();
        let page = self.db.page.get_untracked();
        let sort = self.db.sort.get_untracked();
        let filter = self.db.filter.get_untracked();
        self.set_console_sql({
            let by = sort.as_ref().map(|(c, a)| (c.as_str(), *a));
            let f = filter.as_ref().map(|(c, v)| (c.as_str(), v.as_deref()));
            e_db::browse_sql(&engine, &table, f, by, DB_PAGE, page * DB_PAGE)
        });
        self.db.result_loading.set(true);
        self.db.result_error.set(None);
        // What each answer below is for. The user may have moved on to
        // another table (or connection, or a console run) by the time it
        // comes, and then it is dropped.
        let gen = self.db_next_browse();
        let shown = {
            let state = *self;
            let (key, table) = (key.clone(), table.clone());
            move || state.db_browse_current(gen, &key, &table)
        };
        // Total row count (with the active filter) for pagination, loaded in
        // parallel with the page.
        let send_count = create_ext_action(self.cx, {
            let state = *self;
            let shown = shown.clone();
            move |n: i64| {
                if shown() {
                    state.db.total_rows.set(Some(n));
                }
            }
        });
        {
            let (conn, engine, table, filter) =
                (conn.clone(), engine.clone(), table.clone(), filter.clone());
            std::thread::spawn(move || {
                let f = filter.as_ref().map(|(c, v)| (c.as_str(), v.as_deref()));
                send_count(e_db::count_rows(&conn, &engine, &table, f).unwrap_or(0));
            });
        }
        let send = create_ext_action(self.cx, {
            let state = *self;
            move |res: Result<e_db::QueryResult, String>| {
                if shown() {
                    state.db_apply_result(res);
                }
            }
        });
        std::thread::spawn(move || {
            let by = sort.as_ref().map(|(c, a)| (c.as_str(), *a));
            let f = filter.as_ref().map(|(c, v)| (c.as_str(), v.as_deref()));
            let sql = e_db::browse_sql(&engine, &table, f, by, DB_PAGE, page * DB_PAGE);
            send(e_db::query(&conn, &sql, DB_PAGE));
        });
    }

    /// Jump to a specific 0-based page (clamped to the known range) and reload.
    pub fn db_goto_page(&self, page: usize) {
        let max_page = self
            .db
            .total_rows
            .get_untracked()
            .map(|n| ((n.max(1) as usize).div_ceil(DB_PAGE)).saturating_sub(1))
            .unwrap_or(page);
        let target = page.min(max_page);
        if target == self.db.page.get_untracked() {
            return;
        }
        self.db.page.set(target);
        self.db_reload_table();
    }

    /// Filter the current table to the value in the cell open in the edit
    /// overlay (WHERE col = value / IS NULL), then reload.
    pub fn db_filter_to_cell(&self) {
        let Some((row, col, column)) = self.db.edit.get_untracked() else {
            return;
        };
        let value = self.db.result.with_untracked(|r| {
            r.as_ref().and_then(|r| {
                r.rows
                    .get(row)
                    .and_then(|row| row.get(col))
                    .cloned()
                    .flatten()
            })
        });
        self.db.edit.set(None);
        self.db.filter.set(Some((column, value)));
        self.db.page.set(0);
        self.db_reload_table();
    }

    /// Clear the active data-view filter and reload.
    pub fn db_clear_filter(&self) {
        if self.db.filter.get_untracked().is_none() {
            return;
        }
        self.db.filter.set(None);
        self.db.page.set(0);
        self.db_reload_table();
    }

    /// Open the "insert row" dialog, one field per column of the current table.
    pub fn db_begin_insert(&self) {
        if self.db.result_table.get_untracked().is_none() {
            return;
        }
        // Block on a read-only (production) connection up front.
        if let Some((entry, ..)) = self.db_edit_target() {
            if entry.read_only.get_untracked() {
                Self::notify(
                    "Read-only: this connection is protected from writes. Toggle read-only off \
                     in the Database panel to insert.",
                );
                return;
            }
        }
        let fields: Vec<InsertField> = self.db.columns.with_untracked(|cols| {
            cols.iter()
                .map(|c| InsertField {
                    name: c.name.clone(),
                    data_type: c.data_type.clone(),
                    nullable: c.nullable,
                    value: self.cx.create_rw_signal(String::new()),
                    // Nullable non-PK columns default to NULL; others start blank
                    // so DB defaults / auto-increment apply.
                    is_null: self.cx.create_rw_signal(c.nullable && c.key != "PRI"),
                })
                .collect()
        });
        if fields.is_empty() {
            Self::notify("Insert: no column metadata — open a table first");
            return;
        }
        self.db.insert_fields.set(fields);
        self.db.insert_open.set(true);
    }

    pub fn db_cancel_insert(&self) {
        self.db.insert_open.set(false);
    }

    /// Stage the row built in the dialog, to be inserted on Submit. Columns
    /// left blank (and not marked NULL) are omitted so database defaults /
    /// auto-increment apply.
    pub fn db_commit_insert(&self) {
        let Some((entry, _, table, engine)) = self.db_edit_target() else {
            return;
        };
        if entry.read_only.get_untracked() {
            Self::notify("Read-only: this connection is protected from writes.");
            self.db.insert_open.set(false);
            return;
        }
        let values: Vec<(String, Option<String>)> = self.db.insert_fields.with_untracked(|fs| {
            fs.iter()
                .filter_map(|f| {
                    if f.is_null.get_untracked() {
                        Some((f.name.clone(), None))
                    } else {
                        let v = f.value.get_untracked();
                        if v.is_empty() {
                            None
                        } else {
                            Some((f.name.clone(), Some(v)))
                        }
                    }
                })
                .collect()
        });
        if values.is_empty() {
            Self::notify("Insert: fill at least one column");
            return;
        }
        let sql = e_db::insert_sql(&engine, &table, &values);
        // The reverse is a DELETE by primary key, when the dialog gave every
        // key column a value; an auto-increment key isn't known until the
        // insert has run, so that row has no reverse.
        let pk_names: Vec<String> = self.db.columns.with_untracked(|cols| {
            cols.iter()
                .filter(|c| c.key == "PRI")
                .map(|c| c.name.clone())
                .collect()
        });
        let reverse =
            insert_pk(&pk_names, &values).map(|pk| e_db::delete_sql(&engine, &table, &pk));
        // Staged with the table's other changes, and inserted with them on
        // Submit — in one transaction, through the confirmation (DB-303).
        if !self.db_claim_pending_target(&entry.key(), &table) {
            return;
        }
        self.db.insert_open.set(false);
        self.db.pending_inserts.update(|v| {
            v.push(crate::state::PendingInsert {
                values,
                sql,
                reverse,
            })
        });
    }

    /// Toggle the sort on a column (asc → desc → off) and reload.
    pub fn db_sort_by(&self, col: String) {
        let next = match self.db.sort.get_untracked() {
            Some((c, true)) if c == col => Some((col, false)),
            Some((c, false)) if c == col => None,
            _ => Some((col, true)),
        };
        self.db.sort.set(next);
        self.db.page.set(0);
        self.db_reload_table();
    }

    /// Move to the next/previous page when browsing a table.
    pub fn db_page_by(&self, delta: i64) {
        let cur = self.db.page.get_untracked() as i64;
        let next = (cur + delta).max(0) as usize;
        if next == self.db.page.get_untracked() {
            return;
        }
        // Don't page past the end (a short page means we're at the last one).
        if delta > 0 {
            let len = self
                .db
                .result
                .with_untracked(|r| r.as_ref().map(|r| r.rows.len()).unwrap_or(0));
            if len < DB_PAGE {
                return;
            }
        }
        self.db.page.set(next);
        self.db_reload_table();
    }

    /// Whether the results view still shows `table` of connection `key`.
    fn db_showing(&self, key: &str, table: &str) -> bool {
        self.db
            .result_key
            .with_untracked(|k| k.as_deref() == Some(key))
            && self
                .db
                .result_table
                .with_untracked(|t| t.as_deref() == Some(table))
    }

    /// Start a new browse (see `DbPanel::browse_gen`): answers to earlier
    /// ones are dropped from now on. Returns the new one's number.
    fn db_next_browse(&self) -> u64 {
        self.db.browse_gen.update(|g| *g += 1);
        self.db.browse_gen.get_untracked()
    }

    /// Whether browse `gen` of `table` on connection `key` is still the one
    /// shown.
    fn db_browse_current(&self, gen: u64, key: &str, table: &str) -> bool {
        self.db.browse_gen.get_untracked() == gen && self.db_showing(key, table)
    }

    /// A fresh stop flag for a console run or search about to start, set by
    /// Cancel (see `DbPanel::run_stop`).
    fn db_new_run_stop(&self) -> Arc<std::sync::atomic::AtomicBool> {
        // A new run supersedes the last: its statements left (or tables left
        // to search) aren't wanted any more, so it stops too.
        self.db
            .run_stop
            .with_untracked(|s| s.store(true, std::sync::atomic::Ordering::SeqCst));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.db.run_stop.set(stop.clone());
        stop
    }

    pub fn db_set_subview(&self, view: &str) {
        self.db.subview.set(view.to_string());
    }

    fn db_load_columns(&self, entry: DbEntry, table: String) {
        let Some(conn) = entry.conn.get_untracked() else {
            return;
        };
        let send = create_ext_action(self.cx, {
            let state = *self;
            let (key, table) = (entry.key(), table.clone());
            move |(cols, idx): (Vec<e_db::ColumnInfo>, Vec<e_db::IndexInfo>)| {
                // The columns (and with them the primary key edits are made
                // by) of a table no longer shown must not land on the one
                // that is.
                if !state.db_showing(&key, &table) {
                    return;
                }
                state.db.columns.set(cols);
                state.db.indexes.set(idx);
                // The first page may have come before the primary key was known.
                state.db_load_pending_view();
            }
        });
        let is_view = entry.views.with_untracked(|v| v.contains(&table));
        self.db.is_view.set(is_view);
        // Its DDL apart, so the columns (which editing needs) don't wait.
        let show_ddl = create_ext_action(self.cx, {
            let state = *self;
            let (key, table) = (entry.key(), table.clone());
            move |ddl: String| {
                if state.db_showing(&key, &table) {
                    state.db.ddl.set(Some(ddl));
                }
            }
        });
        let ddl_conn = conn.clone();
        let ddl_table = table.clone();
        std::thread::spawn(move || {
            let cols = e_db::columns(&conn, &table).unwrap_or_default();
            let idx = e_db::indexes(&conn, &table).unwrap_or_default();
            send((cols, idx));
        });
        std::thread::spawn(move || {
            let ddl = if is_view {
                e_db::view_definition(&ddl_conn, &ddl_table)
            } else {
                e_db::table_ddl(&ddl_conn, &ddl_table)
            };
            show_ddl(ddl.unwrap_or_default());
        });
    }

    /// Test the current add-form connection without saving it.
    pub fn db_test_connection(&self) {
        let cfg = self.db.form.get_untracked().to_config();
        self.db.test_state.set("testing".into());
        let send = create_ext_action(self.cx, {
            let state = *self;
            move |res: Result<(), String>| {
                state.db.test_state.set(match res {
                    Ok(()) => "ok".into(),
                    Err(e) => e,
                });
            }
        });
        std::thread::spawn(move || {
            send(e_db::test(&cfg));
        });
    }

    /// Begin editing an existing connection (load it into the form).
    pub fn db_start_edit(&self, entry: DbEntry) {
        let c = &entry.config;
        self.db.form.set(DbForm {
            id: c.id.clone(),
            secrets_in_keychain: c.secrets_in_keychain,
            engine: c.engine.clone(),
            host: c.host.clone(),
            port: if c.port == 0 {
                String::new()
            } else {
                c.port.to_string()
            },
            database: c.database.clone(),
            username: c.username.clone(),
            password: c.password.clone(),
            path: c.path.clone(),
            group: c.group.clone(),
            label: c.label.clone(),
            use_ssh: c.use_ssh,
            ssh_host: c.ssh_host.clone(),
            ssh_port: if c.ssh_port == 0 {
                "22".into()
            } else {
                c.ssh_port.to_string()
            },
            ssh_user: c.ssh_user.clone(),
            ssh_auth: if c.ssh_auth.is_empty() {
                "key".into()
            } else {
                c.ssh_auth.clone()
            },
            ssh_password: c.ssh_password.clone(),
            ssh_key_path: c.ssh_key_path.clone(),
            ssh_passphrase: c.ssh_passphrase.clone(),
            tls: c.tls.clone(),
            tls_ca: c.tls_ca.clone(),
            env_label: c.env_label.clone(),
        });
        self.db.editing_key.set(Some(entry.key()));
        self.db.test_state.set(String::new());
        self.db.adding.set(true);
    }

    /// Save the add/edit form: either add a new connection or replace one.
    pub fn db_submit_form(&self) {
        let cfg = self.db.form.get_untracked().to_config();
        if let Some(old_key) = self.db.editing_key.get_untracked() {
            self.db.conns.update(|c| c.retain(|e| e.key() != old_key));
            self.db.editing_key.set(None);
        }
        self.db.form.set(DbForm::default());
        self.db.adding.set(false);
        self.db.test_state.set(String::new());
        self.db_add_config(cfg);
    }

    /// Export the current result grid to a file in the given format.
    pub fn db_export(&self, fmt: crate::db_export::Format) {
        let Some(result) = self.db.result.get_untracked() else {
            return;
        };
        if result.columns.is_empty() {
            return;
        }
        let table = self
            .db
            .result_table
            .get_untracked()
            .unwrap_or_else(|| "results".into());
        let engine = self.db_result_engine();
        let ext = fmt.extension();
        let opts = floem::file::FileDialogOptions::new()
            .title("Export results")
            .default_name(format!("{table}.{ext}"));
        floem::action::save_as(opts, move |info| {
            let Some(path) = info.and_then(|i| i.path.into_iter().next()) else {
                return;
            };
            let out = crate::db_export::format(&result, fmt, &table, &engine);
            match std::fs::write(&path, out) {
                Ok(()) => Self::notify(&format!(
                    "Exported {} row{} as {}",
                    result.rows.len(),
                    if result.rows.len() == 1 { "" } else { "s" },
                    fmt.name()
                )),
                Err(e) => Self::notify(&format!("Export: could not write the file ({e})")),
            }
        });
    }

    /// The engine of the connection the shown result comes from, for SQL
    /// written from it (MySQL's quoting when none is known).
    fn db_result_engine(&self) -> String {
        let key = self
            .db
            .active_tab
            .get_untracked()
            .and_then(|i| {
                self.db
                    .result_tabs
                    .with_untracked(|t| t.get(i)?.key.clone())
            })
            .or_else(|| self.db.result_key.get_untracked());
        key.and_then(|k| {
            self.db.conns.with_untracked(|c| {
                c.iter()
                    .find(|e| e.key() == k)
                    .map(|e| e.config.engine.clone())
            })
        })
        .unwrap_or_default()
    }

    /// Take a snapshot of a local database (DB-704): a SQLite copy, or a
    /// mysqldump / pg_dump, in its own folder under `~/.config/e/snapshots/`.
    pub fn db_snapshot(&self, entry: DbEntry) {
        if !entry.config.environment().is_local() {
            Self::notify("Snapshot: only available on local connections");
            return;
        }
        let Some(root) = crate::config::snapshot_dir() else {
            return;
        };
        let cfg = entry.config.clone();
        let name = cfg.display_name();
        let send =
            create_ext_action(
                self.cx,
                move |res: Result<std::path::PathBuf, String>| match res {
                    Ok(path) => Self::notify(&format!(
                        "Snapshot of {name} taken ({})",
                        file_size(std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0))
                    )),
                    Err(e) => Self::notify(&format!("Snapshot failed: {e}")),
                },
            );
        std::thread::spawn(move || send(e_db::take_snapshot(&cfg, &root)));
    }

    /// A local connection's snapshots, newest first.
    pub(crate) fn db_snapshots(&self, entry: &DbEntry) -> Vec<e_db::Snapshot> {
        if !entry.config.environment().is_local() {
            return Vec::new();
        }
        crate::config::snapshot_dir()
            .map(|root| e_db::list_snapshots(&root, &entry.config))
            .unwrap_or_default()
    }

    /// Put a local database back as `snap` has it, once confirmed. A
    /// snapshot of it as it is now is taken first, so the restore can be
    /// undone the same way.
    pub fn db_restore_snapshot(&self, entry: DbEntry, snap: e_db::Snapshot) {
        if !entry.config.environment().is_local() {
            Self::notify("Restore: only available on local connections");
            return;
        }
        if entry.read_only.get_untracked() {
            Self::notify(
                "Read-only: this connection is protected from writes. Toggle read-only off \
                 in the Database panel to restore it.",
            );
            return;
        }
        let name = entry.config.display_name();
        let when = ago(crate::state::now_ms_epoch() / 1000, snap.taken);
        let state = *self;
        self.ui.confirm.set(Some(crate::dialogs::Confirm {
            title: format!("Restore {name} to its snapshot from {when}?"),
            detail: "The database goes back to how it was then, and what changed since is \
                     lost. A snapshot of it as it is now is taken first, so this can be \
                     undone the same way."
                .into(),
            action: "Restore",
            on_confirm: std::rc::Rc::new(move || state.db_run_restore(entry.clone(), snap.clone())),
        }));
    }

    fn db_run_restore(&self, entry: DbEntry, snap: e_db::Snapshot) {
        let Some(root) = crate::config::snapshot_dir() else {
            return;
        };
        let cfg = entry.config.clone();
        let name = cfg.display_name();
        let state = *self;
        let done = create_ext_action(self.cx, move |res: Result<(), String>| match res {
            Ok(()) => {
                Self::notify(&format!(
                    "{name} restored — its state before is in a new snapshot"
                ));
                // Its tables and columns may not be what they were.
                entry.schema.set(None);
                state.db_refresh_tables(entry.clone());
                if state.db.result_key.get_untracked().as_deref() == Some(entry.key().as_str())
                    && state.db.result_table.get_untracked().is_some()
                {
                    state.db_reload_table();
                }
            }
            Err(e) => Self::notify(&format!("Restore failed: {e}")),
        });
        std::thread::spawn(move || {
            let res = e_db::take_snapshot(&cfg, &root)
                .map_err(|e| {
                    format!("couldn't take a snapshot first, so nothing was restored ({e})")
                })
                .and_then(|_| e_db::restore_snapshot(&cfg, &snap.path));
            done(res);
        });
    }

    /// Seed `count` rows into the current table via its Eloquent factory
    /// (through Tinker). Local connections only (uses the app's configured DB).
    pub fn db_seed_table(&self, count: usize) {
        let (Some(key), Some(table)) = (
            self.db.result_key.get_untracked(),
            self.db.result_table.get_untracked(),
        ) else {
            return;
        };
        let local = self
            .db
            .conns
            .with_untracked(|c| {
                c.iter()
                    .find(|e| e.key() == key)
                    .map(|e| e.config.environment())
            })
            .map(|env| env.is_local())
            .unwrap_or(false);
        if !local {
            Self::notify("Seed: only available on local connections");
            return;
        }
        if !self.db_table_is_apps() {
            Self::notify("Seed: the factory writes to the app's own database (.env), not this one");
            return;
        }
        let model = crate::laravel::model_class_from_table(&table);
        let code = format!(
            "\\{model}::factory()->count({count})->create(); \
             echo 'Seeded {count} row(s) via {model}';"
        );
        self.run_tinker(code);
        // The factory writes through the app; refresh the grid shortly after.
        let state = *self;
        floem::action::exec_after(std::time::Duration::from_millis(1500), move |_| {
            state.db_reload_table();
        });
    }

    /// The shown table's model, and where its factory is or would be:
    /// `database/factories/{Model}Factory.php`. `None` outside a Laravel
    /// project.
    pub(crate) fn db_factory_path(&self) -> Option<(String, std::path::PathBuf)> {
        let table = self.db.result_table.get_untracked()?;
        let root = self.root.get_untracked();
        if !root.join("artisan").is_file() {
            return None;
        }
        let model = e_laravel::codegen::model_name(&table);
        let path = root
            .join("database/factories")
            .join(format!("{model}Factory.php"));
        Some((model, path))
    }

    /// Whether the shown table's connection is local.
    pub(crate) fn db_table_is_local(&self) -> bool {
        let Some(key) = self.db.result_key.get_untracked() else {
            return false;
        };
        self.db.conns.with_untracked(|c| {
            c.iter()
                .find(|e| e.key() == key)
                .is_some_and(|e| e.config.environment().is_local())
        })
    }

    /// Whether the shown table's connection is the app's own database, the
    /// one `.env` names — where Tinker, and so seeding, writes.
    pub(crate) fn db_table_is_apps(&self) -> bool {
        let Some(key) = self.db.result_key.get_untracked() else {
            return false;
        };
        let Some(app) = e_db::from_env(&self.root.get_untracked()) else {
            return false;
        };
        self.db.conns.with_untracked(|c| {
            c.iter()
                .find(|e| e.key() == key)
                .is_some_and(|e| same_database(&e.config, &app))
        })
    }

    /// Write a factory for the shown table from its columns and foreign
    /// keys, and open it (DB-802). An existing factory is opened, not
    /// replaced.
    pub fn db_generate_factory(&self) {
        let Some((model, path)) = self.db_factory_path() else {
            Self::notify("Generate factory: not a Laravel project");
            return;
        };
        if self
            .db
            .result_table
            .get_untracked()
            .is_none_or(|t| !e_laravel::codegen::valid_table(&t))
        {
            Self::notify("Generate factory: the table's name can't be written into PHP as it is");
            return;
        }
        if path.is_file() {
            self.open_path(path);
            return;
        }
        let (Some(key), Some(table)) = (
            self.db.result_key.get_untracked(),
            self.db.result_table.get_untracked(),
        ) else {
            return;
        };
        let Some(conn) = self
            .db
            .conns
            .with_untracked(|c| c.iter().find(|e| e.key() == key).cloned())
            .and_then(|e| e.conn.get_untracked())
        else {
            return;
        };
        let root = self.root.get_untracked();
        let state = *self;
        let send = create_ext_action(self.cx, move |written: Result<(), String>| match written {
            Ok(()) => {
                state.fs_rev.update(|r| *r += 1);
                state.open_path(path.clone());
                let model_file = root.join("app/Models").join(format!("{model}.php"));
                let has_factory = std::fs::read_to_string(&model_file)
                    .map(|m| m.contains("HasFactory"))
                    .unwrap_or(false);
                Self::notify(&if has_factory {
                    format!("{model}Factory written — Seed ▾ can use it now")
                } else {
                    format!(
                        "{model}Factory written — add `use HasFactory;` to {model} to seed with it"
                    )
                });
            }
            Err(e) => Self::notify(&format!("Generate factory: {e}")),
        });
        let factory_path = self.db_factory_path().map(|(_, p)| p);
        std::thread::spawn(move || {
            let written = (|| {
                let cols = e_db::columns(&conn, &table)?;
                let fks = e_db::foreign_keys(&conn).unwrap_or_default();
                let code = e_laravel::codegen::generate_factory(&table, &cols, &fks);
                let path = factory_path.ok_or("not a Laravel project")?;
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                }
                std::fs::write(&path, code).map_err(|e| e.to_string())
            })();
            send(written);
        });
    }

    /// Load the active database's foreign-key relationships and open the schema-
    /// relationships (ERD) panel (DB-207). Works for tables without models.
    pub fn db_show_erd(&self) {
        let Some(entry) = self.db_active_entry("Relationships") else {
            return;
        };
        let Some(conn) = entry.conn.get_untracked() else {
            return;
        };
        self.db.erd_open.set(true);
        let sig = self.db.erd;
        let send = create_ext_action(self.cx, move |fks: Vec<e_db::ForeignKey>| sig.set(fks));
        std::thread::spawn(move || {
            send(e_db::foreign_keys(&conn).unwrap_or_default());
        });
    }

    /// Search every table's text columns for a value, showing one result tab per
    /// table that matches (DB-805). Cancellable via the run generation.
    pub fn db_search_all(&self) {
        let needle = self.db.search_query.get_untracked().trim().to_string();
        if needle.is_empty() {
            return;
        }
        let Some(entry) = self.db_active_entry("Search") else {
            return;
        };
        let Some(conn) = entry.conn.get_untracked() else {
            return;
        };
        let engine = entry.config.engine.clone();
        let key = entry.key();
        // The search's connection is the one Cancel stops it on.
        self.db.result_key.set(Some(key.clone()));
        self.db.result_title.set(format!(
            "{} · search “{needle}”",
            entry.config.display_name()
        ));
        self.db.result_loading.set(true);
        self.db.result_open.set(true);
        self.db.run_gen.update(|g| *g += 1);
        let gen = self.db.run_gen.get_untracked();
        self.db_next_browse();
        let stop = self.db_new_run_stop();
        let state = *self;
        let send = create_ext_action(self.cx, move |hits: Vec<(String, e_db::QueryResult)>| {
            if state.db.run_gen.get_untracked() != gen {
                return;
            }
            state.db_show_search_results(key.clone(), hits);
        });
        std::thread::spawn(move || {
            // Cancel stops it before the next table.
            let stopped = || stop.load(std::sync::atomic::Ordering::SeqCst);
            send(e_db::search_all(&conn, &engine, &needle, DB_PAGE, stopped).unwrap_or_default());
        });
    }

    /// Show cross-table search hits as read-only result tabs (one per table).
    /// With no hits, whatever was shown stays.
    fn db_show_search_results(&self, key: String, hits: Vec<(String, e_db::QueryResult)>) {
        self.db.result_loading.set(false);
        if hits.is_empty() {
            Self::notify("No matches in any table");
            return;
        }
        let tabs = hits
            .into_iter()
            .map(|(table, res)| crate::state::ResultTab {
                title: table,
                result: Some(res),
                error: None,
                pinned: false,
                key: Some(key.clone()),
                explain: Vec::new(),
                sql: None,
                advice: None,
                plan: None,
            })
            .collect();
        self.db_show_new_tabs(tabs);
    }

    /// The connected database an agent asks for: the one it names, or else
    /// the panel's active one (or the only one connected). With several and
    /// none chosen, an error that says to name one rather than a guess.
    pub(crate) fn db_for_agent(&self, name: Option<&str>) -> Result<DbEntry, String> {
        let connected: Vec<DbEntry> = self.db.conns.with_untracked(|cs| {
            cs.iter()
                .filter(|e| e.conn.get_untracked().is_some())
                .cloned()
                .collect()
        });
        if let Some(n) = name {
            return connected
                .into_iter()
                .find(|e| e.config.display_name() == n)
                .ok_or_else(|| format!("no connected database named {n}"));
        }
        let key = self.db.result_key.get_untracked();
        let keys: Vec<String> = connected.iter().map(|e| e.key()).collect();
        match pick_active(key.as_deref(), &keys) {
            Some(i) => Ok(connected[i].clone()),
            None if connected.is_empty() => Err("no connected database".into()),
            None => Err(format!(
                "several databases are connected and none is active; name one in \
                 \"connection\": {}",
                connected
                    .iter()
                    .map(|e| e.config.display_name())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    /// The connection the panel works with: the one its results come from,
    /// or, if that isn't connected, the only connected one. With several
    /// connected and none chosen, says so (as `what`) and returns `None`
    /// rather than guess.
    pub(crate) fn db_active_entry(&self, what: &str) -> Option<DbEntry> {
        let key = self.db.result_key.get_untracked();
        let found = self.db.conns.with_untracked(|cs| {
            let connected: Vec<&DbEntry> = cs
                .iter()
                .filter(|e| e.conn.get_untracked().is_some())
                .collect();
            pick_active(
                key.as_deref(),
                &connected.iter().map(|e| e.key()).collect::<Vec<_>>(),
            )
            .map(|i| connected[i].clone())
            .ok_or(connected.len())
        });
        match found {
            Ok(entry) => Some(entry),
            Err(0) => {
                Self::notify(&format!(
                    "{what}: connect a database in the Database panel (⌘3) first"
                ));
                None
            }
            Err(_) => {
                Self::notify(&format!(
                    "{what}: several databases are connected — open a table in the one you \
                     mean first"
                ));
                None
            }
        }
    }

    /// Generate a Laravel migration scaffold for structure changes to the
    /// current table and open it, instead of running DDL directly (DB-803).
    pub fn db_new_migration_for_table(&self) {
        let Some(table) = self.db.result_table.get_untracked() else {
            return;
        };
        if !crate::codegen::valid_table(&table) {
            return;
        }
        let root = self.root.get_untracked();
        let dir = root.join("database/migrations");
        if !dir.is_dir() {
            Self::notify("Not a Laravel project (no database/migrations)");
            return;
        }
        let ts = crate::codegen::migration_timestamp();
        let file = dir.join(format!("{ts}_update_{table}_table.php"));
        let content = crate::codegen::change_table_migration(&table);
        if std::fs::write(&file, content).is_ok() {
            self.open_path(file.clone());
            Self::notify(&format!("Created {}", file.display()));
        } else {
            Self::notify("Could not write the migration");
        }
    }

    /// Copy the current table's CREATE DDL to the clipboard (DB-204).
    pub fn db_copy_ddl(&self) {
        // Read with the columns, if it has come.
        if let Some(ddl) = self.db.ddl.get_untracked().filter(|d| !d.trim().is_empty()) {
            let _ = floem::Clipboard::set_contents(ddl);
            Self::notify(if self.db.is_view.get_untracked() {
                "View definition copied to clipboard"
            } else {
                "DDL copied to clipboard"
            });
            return;
        }
        let (Some(key), Some(table)) = (
            self.db.result_key.get_untracked(),
            self.db.result_table.get_untracked(),
        ) else {
            return;
        };
        let Some(entry) = self
            .db
            .conns
            .with_untracked(|c| c.iter().find(|e| e.key() == key).cloned())
        else {
            return;
        };
        let Some(conn) = entry.conn.get_untracked() else {
            return;
        };
        let send = create_ext_action(self.cx, move |ddl: String| {
            if ddl.is_empty() {
                Self::notify("No DDL available");
            } else {
                let _ = floem::Clipboard::set_contents(ddl);
                Self::notify("DDL copied to clipboard");
            }
        });
        std::thread::spawn(move || {
            send(e_db::table_ddl(&conn, &table).unwrap_or_default());
        });
    }

    /// Copy the `CREATE INDEX` that makes `ix` on the shown table (DB-204).
    pub fn db_copy_index(&self, ix: &e_db::IndexInfo) {
        let Some(table) = self.db.result_table.get_untracked() else {
            return;
        };
        let _ = floem::Clipboard::set_contents(index_ddl(&self.db_result_engine(), &table, ix));
        Self::notify(&format!("CREATE INDEX {} copied to clipboard", ix.name));
    }

    /// Pick a CSV file to import into the current table, and open the
    /// import dialog on it: its columns mapped by name, a preview, and what
    /// doesn't fit the table (DB-503).
    pub fn db_import_csv(&self) {
        let (Some(key), Some(table)) = (
            self.db.result_key.get_untracked(),
            self.db.result_table.get_untracked(),
        ) else {
            Self::notify("Import: open a table first");
            return;
        };
        let Some(entry) = self
            .db
            .conns
            .with_untracked(|c| c.iter().find(|e| e.key() == key).cloned())
        else {
            return;
        };
        if entry.read_only.get_untracked() {
            Self::notify(
                "Read-only: this connection is protected from writes. Toggle read-only off \
                 in the Database panel to import.",
            );
            return;
        }
        let columns = self.db.columns.get_untracked();
        if columns.is_empty() {
            Self::notify("Import: table columns not loaded yet");
            return;
        }
        let state = *self;
        let opts = floem::file::FileDialogOptions::new().title(format!("Import CSV into {table}"));
        floem::action::open_file(opts, move |info| {
            let Some(path) = info.and_then(|i| i.path.into_iter().next()) else {
                return;
            };
            // Read and parsed off the UI thread: a large file would freeze it.
            let key = key.clone();
            let table = table.clone();
            let entry = entry.clone();
            let columns = columns.clone();
            let file = path.clone();
            let opened = create_ext_action(
                state.cx,
                move |parsed: Result<(char, Vec<Vec<String>>), String>| {
                    state.db_open_import(&entry, &key, &table, &columns, path.clone(), parsed)
                },
            );
            std::thread::spawn(move || {
                opened(
                    std::fs::read(&file)
                        .map_err(|e| e.to_string())
                        .map(|bytes| {
                            let text = String::from_utf8_lossy(&bytes);
                            let delimiter = crate::db_import::sniff_delimiter(&text);
                            (
                                delimiter,
                                crate::db_import::parse_csv_with(&text, delimiter),
                            )
                        }),
                );
            });
        });
    }

    /// The import dialog on a CSV file read and parsed.
    fn db_open_import(
        &self,
        entry: &DbEntry,
        key: &str,
        table: &str,
        columns: &[e_db::ColumnInfo],
        path: std::path::PathBuf,
        parsed: Result<(char, Vec<Vec<String>>), String>,
    ) {
        let state = *self;
        {
            let (delimiter, mut rows) = match parsed {
                Ok(p) => p,
                Err(e) => {
                    Self::notify(&format!("Import: could not read the file ({e})"));
                    return;
                }
            };
            if rows.is_empty() {
                Self::notify("Import: the file is empty");
                return;
            }
            let header = rows.remove(0);
            let names: Vec<String> = columns.iter().map(|c| c.name.clone()).collect();
            let mapping = crate::db_import::auto_map(&header, &names);
            state.db.import.set(Some(crate::state::DbImport {
                key: key.to_string(),
                table: table.to_string(),
                engine: entry.config.engine.clone(),
                env: entry.config.environment(),
                file: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                delimiter,
                header,
                rows: Arc::new(rows),
                columns: columns.to_vec(),
                mapping: state.cx.create_rw_signal(mapping),
                empty_default: state.cx.create_rw_signal(false),
            }));
        }
    }

    pub fn db_cancel_import(&self) {
        self.db.import.set(None);
    }

    /// Import the CSV as the dialog maps it: its `INSERT`s go to the
    /// confirmation dialog, and run in one transaction from there.
    pub fn db_commit_import(&self) {
        let Some(imp) = self.db.import.get_untracked() else {
            return;
        };
        let mapping = imp.mapping.get_untracked();
        let empty_default = imp.empty_default.get_untracked();
        let problems = crate::db_import::problems(
            &imp.rows,
            imp.header.len(),
            &mapping,
            &imp.columns,
            empty_default,
        );
        if let Some(p) = problems.iter().find(|p| p.blocking) {
            Self::notify(&format!("Import: {}", p.text));
            return;
        }
        let stmts = match crate::db_import::build_inserts(
            &imp.engine,
            &imp.table,
            &imp.rows,
            &mapping,
            empty_default,
        ) {
            Ok(stmts) => stmts,
            Err(e) => {
                Self::notify(&format!("Import: {e}"));
                return;
            }
        };
        self.db.import.set(None);
        self.db_ask_confirm(crate::state::DbConfirm {
            verb: "Import".into(),
            statements: stmts.clone(),
            env: imp.env,
            needs_ack: !imp.env.is_local(),
            ack: self.cx.create_rw_signal(false),
            run: crate::state::ConfirmRun::Transaction(crate::state::DbTransaction {
                key: imp.key,
                statements: stmts,
                log: Vec::new(),
                submits_pending: false,
                checks: Vec::new(),
                done: "Imported",
            }),
            counts: self.cx.create_rw_signal(Vec::new()),
        });
    }

    /// Copy the current result to the clipboard in the given format.
    pub fn db_copy_result(&self, fmt: crate::db_export::Format) {
        let Some(result) = self.db.result.get_untracked() else {
            return;
        };
        if result.columns.is_empty() {
            return;
        }
        let table = self
            .db
            .result_table
            .get_untracked()
            .unwrap_or_else(|| "results".into());
        let text = crate::db_export::format(&result, fmt, &table, &self.db_result_engine());
        let _ = floem::Clipboard::set_contents(text);
        Self::notify(&format!(
            "Copied {} row{} as {}",
            result.rows.len(),
            if result.rows.len() == 1 { "" } else { "s" },
            fmt.name()
        ));
    }

    /// Open a blank query editor for a connection.
    pub fn db_new_query(&self, entry: DbEntry) {
        self.db_set_active(entry);
        if self.db.query_text.with_untracked(|q| q.trim().is_empty()) {
            self.set_console_sql("SELECT 1".into());
        }
        self.db.result_open.set(true);
    }

    /// Pick the active connection from the header's switcher: nothing
    /// changes if it already is (a table open on it stays).
    pub fn db_choose_active(&self, entry: DbEntry) {
        let same = self
            .db
            .result_key
            .with_untracked(|k| k.as_deref() == Some(entry.key().as_str()));
        if same {
            if entry.conn.get_untracked().is_none() {
                self.db_connect(entry);
            }
            return;
        }
        self.db_set_active(entry);
    }

    /// Make `entry` the connection the panel works with (DB-104): the
    /// console runs against it, and search, EXPLAIN and relationships use
    /// it. Connects it if need be. A table open on another connection is
    /// left (its tab brings it back), so the grid never shows one
    /// connection's rows under another's name; result tabs stay.
    pub fn db_set_active(&self, entry: DbEntry) {
        if entry.conn.get_untracked().is_none() {
            self.db_connect(entry.clone());
        }
        let key = entry.key();
        self.db_next_browse();
        self.db.edit.set(None);
        self.db.active_tab.set(None);
        self.db.result_table.set(None);
        self.db.columns.set(Vec::new());
        self.db.result_key.set(Some(key));
        self.db
            .result_title
            .set(format!("{} · query", entry.config.display_name()));
        self.db.result.set(None);
        self.db.result_error.set(None);
        self.db.explain_issues.set(Vec::new());
        self.db.selected_cell.set(None);
        self.db.pending_view.set(Default::default());
    }

    /// Run the SQL currently in the query editor against the bound connection.
    /// Run the whole console (all statements).
    pub fn db_run_query(&self) {
        self.run_console_sql(self.db.query_text.get_untracked());
    }

    /// Run just the selected text, or — with no selection — the statement under
    /// the cursor. Falls back to the whole console if neither resolves.
    pub fn run_console_under_cursor(&self) {
        let sql = self.console_sql_under_cursor();
        self.run_console_sql(sql);
    }

    /// EXPLAIN the console's selection or statement under the cursor (⌥⌘↵
    /// there), on the console's connection.
    pub fn explain_console_under_cursor(&self) {
        let sql = self.console_sql_under_cursor();
        if sql.trim().is_empty() {
            return;
        }
        let Some(entry) = self.db_active_entry("EXPLAIN") else {
            return;
        };
        self.db_explain_sql(entry, sql);
    }

    /// The console's selection, or else the statement the caret is in.
    fn console_sql_under_cursor(&self) -> String {
        let text = self.db.query_text.get_untracked();
        let sql = self
            .db
            .console_editor
            .get_untracked()
            .and_then(|editor| {
                let cursor = editor.cursor.get_untracked();
                // Selection wins.
                if let floem::views::editor::core::cursor::CursorMode::Insert(sel) =
                    cursor.mode.clone()
                {
                    if let Some(r) = sel.regions().first() {
                        if r.max() > r.min() {
                            return text.get(r.min()..r.max()).map(|s| s.to_string());
                        }
                    }
                }
                // Otherwise the statement containing the caret.
                let off = cursor.offset();
                let ranges = e_db::split_statement_ranges(&text);
                ranges
                    .iter()
                    .find(|(s, e)| *s <= off && off <= *e)
                    .or_else(|| ranges.last())
                    .map(|(s, e)| text[*s..*e].to_string())
            })
            .unwrap_or(text);
        sql
    }

    /// Gate a console run: prompt for `:param` values first (DB-408), then
    /// destructive/non-local confirmation (DB-702/703).
    fn run_console_sql(&self, sql: String) {
        // Prompt for named parameters if any are present.
        let params = e_db::find_params(&sql);
        if !params.is_empty() {
            let last = self.db.param_last.get_untracked();
            let fields = params
                .into_iter()
                .map(|p| {
                    let init = last.get(&p).cloned().unwrap_or_default();
                    (p, self.cx.create_rw_signal(init))
                })
                .collect();
            self.db
                .params
                .set(Some(crate::state::DbParams { sql, fields }));
            return;
        }
        let Some(entry) = self.db.result_key.get_untracked().and_then(|key| {
            self.db
                .conns
                .with_untracked(|c| c.iter().find(|e| e.key() == key).cloned())
        }) else {
            return;
        };
        let statements = e_db::split_statements(&sql);
        if statements.is_empty() {
            return;
        }
        if entry.read_only.get_untracked() && statements.iter().any(|s| e_db::is_write_statement(s))
        {
            Self::notify(
                "Read-only: this connection is protected from writes. Toggle read-only off \
                 in the Database panel to run statements that write.",
            );
            return;
        }
        let env = entry.config.environment();
        let non_local = !env.is_local();
        let flagged: Vec<String> = statements
            .iter()
            .filter(|s| {
                e_db::is_destructive(s).is_some() || (non_local && e_db::is_write_statement(s))
            })
            .cloned()
            .collect();
        if flagged.is_empty() {
            self.execute_console_sql(sql);
        } else {
            self.db_ask_confirm(crate::state::DbConfirm {
                verb: "Run".into(),
                statements: flagged,
                env,
                needs_ack: non_local,
                ack: self.cx.create_rw_signal(false),
                run: crate::state::ConfirmRun::Console(sql),
                counts: self.cx.create_rw_signal(Vec::new()),
            });
        }
    }

    /// Fill parameters, remember them, and run the substituted SQL.
    pub fn db_params_run(&self) {
        let Some(p) = self.db.params.get_untracked() else {
            return;
        };
        let mut values = std::collections::HashMap::new();
        for (name, sig) in &p.fields {
            values.insert(name.clone(), sig.get_untracked());
        }
        self.db.param_last.update(|m| {
            for (k, v) in &values {
                m.insert(k.clone(), v.clone());
            }
        });
        self.db.params.set(None);
        let engine = self
            .db
            .result_key
            .get_untracked()
            .and_then(|key| {
                self.db.conns.with_untracked(|c| {
                    c.iter()
                        .find(|e| e.key() == key)
                        .map(|e| e.config.engine.clone())
                })
            })
            .unwrap_or_default();
        let sql = e_db::substitute_params(&engine, &p.sql, &values);
        self.run_console_sql(sql);
    }

    pub fn db_params_cancel(&self) {
        self.db.params.set(None);
    }

    /// Confirm and run the pending action (console SQL or a submit transaction).
    pub fn db_confirm_run(&self) {
        let Some(c) = self.db.confirm.get_untracked() else {
            return;
        };
        if c.needs_ack && !c.ack.get_untracked() {
            Self::notify("Tick the acknowledgement to proceed");
            return;
        }
        self.db.confirm.set(None);
        match c.run {
            crate::state::ConfirmRun::Console(sql) => self.execute_console_sql(sql),
            crate::state::ConfirmRun::Transaction(tx) => self.execute_submit(tx),
        }
    }

    /// Dismiss the confirmation dialog without running.
    pub fn db_confirm_cancel(&self) {
        self.db.confirm.set(None);
    }

    /// Run `sql` (which may contain multiple statements) against the active
    /// connection, one result tab per statement. Assumes confirmation (if any)
    /// has already been given.
    fn execute_console_sql(&self, sql: String) {
        let Some(key) = self.db.result_key.get_untracked() else {
            return;
        };
        let Some(entry) = self
            .db
            .conns
            .with_untracked(|c| c.iter().find(|e| e.key() == key).cloned())
        else {
            return;
        };
        let Some(conn) = entry.conn.get_untracked() else {
            self.db.result_error.set(Some("Not connected".into()));
            return;
        };
        // Split into individual statements so each gets its own result tab.
        let statements = e_db::split_statements(&sql);
        if statements.is_empty() {
            return;
        }
        self.db.result_loading.set(true);
        self.db.result_error.set(None);
        // Bump the run generation so any prior in-flight query (or a cancel) is
        // ignored when it returns.
        self.db.run_gen.update(|g| *g += 1);
        let gen = self.db.run_gen.get_untracked();
        // A table page still on its way is no longer wanted either.
        self.db_next_browse();
        let stop = self.db_new_run_stop();
        // Metadata for the query-history log (written off the UI thread).
        let project = self.root.get_untracked().to_string_lossy().into_owned();
        let conn_label = entry.config.display_name();
        let history_path = crate::config::history_db_path();
        let key = entry.key();
        let send = create_ext_action(self.cx, {
            let state = *self;
            move |results: Vec<(String, Result<e_db::QueryResult, String>)>| {
                // Discard if cancelled or superseded by a newer run.
                if state.db.run_gen.get_untracked() != gen {
                    return;
                }
                state.db_build_console_tabs(key.clone(), results);
            }
        });
        std::thread::spawn(move || {
            let mut out = Vec::with_capacity(statements.len());
            for stmt in &statements {
                // After Cancel the statements left aren't run: the one that
                // was running has been stopped, and the rest would go on
                // unseen (their results are dropped).
                if stop.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                let res = e_db::query(&conn, stmt, e_db::MAX_ROWS);
                if let Some(hp) = &history_path {
                    let (rows, dur, ok, err) = match &res {
                        Ok(r) => (
                            r.rows_affected
                                .map(|n| n as i64)
                                .or(Some(r.rows.len() as i64)),
                            r.elapsed_ms as i64,
                            true,
                            None,
                        ),
                        Err(e) => (None, 0, false, Some(e.clone())),
                    };
                    let _ = e_db::history::record(
                        hp,
                        &project,
                        &conn_label,
                        stmt,
                        rows,
                        dur,
                        ok,
                        err.as_deref(),
                    );
                }
                out.push((stmt.clone(), res));
            }
            send(out);
        });
    }

    /// Build console result tabs from a multi-statement run (see
    /// `db_show_new_tabs`).
    fn db_build_console_tabs(
        &self,
        key: String,
        results: Vec<(String, Result<e_db::QueryResult, String>)>,
    ) {
        self.db.result_loading.set(false);
        let tabs = results
            .into_iter()
            .enumerate()
            .map(|(i, (stmt, res))| {
                let (result, error) = match res {
                    Ok(r) => (Some(r), None),
                    Err(e) => (None, Some(e)),
                };
                crate::state::ResultTab {
                    title: format!("Result {}", i + 1),
                    result,
                    error,
                    pinned: false,
                    key: Some(key.clone()),
                    explain: Vec::new(),
                    sql: Some(stmt),
                    advice: None,
                    plan: None,
                }
            })
            .collect();
        self.db_show_new_tabs(tabs);
    }

    /// Put new result tabs in place of the unpinned ones (pinned tabs stay)
    /// and show the first of them.
    fn db_show_new_tabs(&self, new: Vec<crate::state::ResultTab>) {
        let mut tabs = self.db.result_tabs.get_untracked();
        tabs.retain(|t| t.pinned);
        let first_new = tabs.len();
        tabs.extend(new);
        self.db.result_tabs.set(tabs);
        self.db_activate_tab(first_new);
    }

    /// Cancel the in-flight query: free the UI, discard the pending result
    /// when it returns, and ask the server to stop it. On PostgreSQL the
    /// query then ends at once and its connection is free for the next one;
    /// elsewhere it runs to its end in the background.
    pub fn db_cancel_query(&self) {
        self.db.run_gen.update(|g| *g += 1);
        self.db_next_browse();
        // A console run or search stops before its next statement or table.
        self.db
            .run_stop
            .with_untracked(|s| s.store(true, std::sync::atomic::Ordering::SeqCst));
        self.db.result_loading.set(false);
        let conn = self.db.result_key.get_untracked().and_then(|key| {
            self.db.conns.with_untracked(|c| {
                c.iter()
                    .find(|e| e.key() == key)
                    .and_then(|e| e.conn.get_untracked())
            })
        });
        let Some(conn) = conn else {
            Self::notify("Query cancelled");
            return;
        };
        let send = create_ext_action(self.cx, |res: Result<bool, String>| {
            Self::notify(&match res {
                Ok(true) => "Query cancelled".to_string(),
                Ok(false) => {
                    "Query cancelled here; the database finishes it in the background".to_string()
                }
                Err(e) => format!("Query cancelled here, but the database didn't stop it: {e}"),
            });
        });
        std::thread::spawn(move || send(e_db::cancel(&conn)));
    }

    /// Show the result stored in tab `i` in the grid. It isn't the browsed
    /// table's rows, so the grid stops editing that table (its tab brings it
    /// back), and a page of it still on its way is dropped.
    pub fn db_activate_tab(&self, i: usize) {
        let tabs = self.db.result_tabs.get_untracked();
        let Some(tab) = tabs.get(i) else {
            return;
        };
        self.db_next_browse();
        self.db.edit.set(None);
        self.db.result_table.set(None);
        self.db.columns.set(Vec::new());
        self.db.active_tab.set(Some(i));
        // Name what's shown: the tab, not the table left behind.
        let db_name = tab.key.as_ref().and_then(|k| {
            self.db.conns.with_untracked(|c| {
                c.iter()
                    .find(|e| &e.key() == k)
                    .map(|e| e.config.display_name())
            })
        });
        self.db.result_title.set(match db_name {
            Some(name) => format!("{name} · {}", tab.title),
            None => tab.title.clone(),
        });
        self.db.result_key.set(tab.key.clone());
        self.db.result.set(tab.result.clone());
        self.db.result_error.set(tab.error.clone());
        self.db.explain_issues.set(tab.explain.clone());
        self.db.selected_cell.set(None);
        self.db.pending_view.set(Default::default());
    }

    /// Go back from a result tab to the table browsed before it, at the
    /// same page, sort and filter.
    pub fn db_show_browsed(&self) {
        let Some((key, table)) = self.db.browsed.get_untracked() else {
            return;
        };
        if self.db.active_tab.get_untracked().is_none() && self.db_showing(&key, &table) {
            return;
        }
        let entry = self.db.conns.with_untracked(|c| {
            c.iter()
                .find(|e| e.key() == key && e.conn.get_untracked().is_some())
                .cloned()
        });
        let Some(entry) = entry else {
            Self::notify(&format!("{table}: its database is no longer connected"));
            return;
        };
        self.db.edit.set(None);
        self.db.active_tab.set(None);
        self.db.result_key.set(Some(key));
        self.db.result_table.set(Some(table.clone()));
        self.db
            .result_title
            .set(format!("{} · {}", entry.config.display_name(), table));
        self.db.subview.set("data".into());
        self.db.columns.set(Vec::new());
        self.db.explain_issues.set(Vec::new());
        self.db_load_columns(entry, table);
        self.db_reload_table();
    }

    /// Toggle whether tab `i` is pinned (survives the next run).
    pub fn db_toggle_pin(&self, i: usize) {
        self.db.result_tabs.update(|tabs| {
            if let Some(t) = tabs.get_mut(i) {
                t.pinned = !t.pinned;
            }
        });
    }

    /// Close result tab `i`. Closing the one shown shows its neighbour, or
    /// the browsed table when it was the last.
    pub fn db_close_tab(&self, i: usize) {
        let len = self.db.result_tabs.with_untracked(|t| t.len());
        if i >= len {
            return;
        }
        self.db.result_tabs.update(|tabs| {
            tabs.remove(i);
        });
        let active = self.db.active_tab.get_untracked();
        match active_after_close(active, i, len - 1) {
            Some(next) if active == Some(i) => self.db_activate_tab(next),
            Some(next) => self.db.active_tab.set(Some(next)),
            None if active.is_none() => {}
            None => {
                self.db.active_tab.set(None);
                self.db.result.set(None);
                self.db.result_error.set(None);
                self.db.explain_issues.set(Vec::new());
                self.db_show_browsed();
            }
        }
    }

    /// Run the raw SQL string under the cursor (`DB::select("…")`, `->whereRaw`,
    /// migrations' `DB::statement`, …) against a connected database and show the
    /// results in the DB result overlay. Bound to ⌘⏎.
    /// Locate the SQL string under the cursor and a connected database to run it
    /// against. Notifies (and returns `None`) when the pieces aren't in place.
    fn sql_and_conn_under_cursor(&self) -> Option<(DbEntry, String)> {
        let buf = self.active_buffer()?;
        if buf.file.language != Language::Php {
            Self::notify("Run SQL: not a PHP file");
            return None;
        }
        let editor = buf.editor.get_untracked()?;
        let offset = editor.cursor.get_untracked().offset();
        let text = buf.doc.text().to_string();
        let Some((s, e)) = e_core::syntax::php_sql_range_at(&text, offset) else {
            Self::notify(
                "Run SQL: put the cursor inside a DB query string (DB::select, ->whereRaw, …)",
            );
            return None;
        };
        let sql = text.get(s..e).unwrap_or("").trim().to_string();
        if sql.is_empty() {
            return None;
        }
        let entry = self.db_active_entry("Run SQL")?;
        Some((entry, sql))
    }

    pub fn run_sql_under_cursor(&self) {
        let Some((entry, sql)) = self.sql_and_conn_under_cursor() else {
            return;
        };
        self.db.result_key.set(Some(entry.key()));
        self.db.result_title.set(format!(
            "{} · query under cursor",
            entry.config.display_name()
        ));
        self.set_console_sql(sql);
        self.db.result.set(None);
        self.db.result_error.set(None);
        self.db.result_open.set(true);
        self.db_run_query();
    }

    /// Run EXPLAIN on the SQL under the cursor, show the plan in the result
    /// overlay, and flag full scans / missing indexes.
    pub fn explain_sql_under_cursor(&self) {
        let Some((entry, sql)) = self.sql_and_conn_under_cursor() else {
            return;
        };
        self.db_explain_sql(entry, sql);
    }

    /// Run EXPLAIN on `sql` against a specific connection, showing the plan in
    /// the result overlay and flagging full scans / missing indexes.
    pub fn db_explain_sql(&self, entry: DbEntry, sql: String) {
        let Some(conn) = entry.conn.get_untracked() else {
            Self::notify("EXPLAIN: not connected");
            return;
        };
        let engine = entry.config.engine.clone();
        self.db.result_key.set(Some(entry.key()));
        self.db
            .result_title
            .set(format!("{} · EXPLAIN", entry.config.display_name()));
        self.db.result_loading.set(true);
        self.db.open.set(true);
        self.db.result_open.set(true);
        self.db.run_gen.update(|g| *g += 1);
        let gen = self.db.run_gen.get_untracked();
        let key = entry.key();
        let sql_for_tab = sql.clone();
        let state = *self;
        // The plan is a result of its own, in a tab, with its findings as a
        // banner over it; it isn't rows of the browsed table.
        let engine_for_advice = engine.clone();
        let sql_for_advice = sql.clone();
        let send = create_ext_action(self.cx, move |(res, advice, plan): ExplainedPlan| {
            if state.db.run_gen.get_untracked() != gen {
                return;
            }
            state.db.result_loading.set(false);
            let explain = match &res {
                Ok(plan) => e_db::analyze_explain(&engine, plan),
                Err(_) => Vec::new(),
            };
            let (result, error) = match res {
                Ok(r) => (Some(r), None),
                Err(e) => (None, Some(e)),
            };
            state.db_show_new_tabs(vec![crate::state::ResultTab {
                title: "EXPLAIN".into(),
                result,
                error,
                pinned: false,
                key: Some(key.clone()),
                explain,
                sql: Some(sql_for_tab.clone()),
                advice,
                plan,
            }]);
        });
        std::thread::spawn(move || {
            let res = e_db::explain(&conn, &sql);
            let advice = res
                .as_ref()
                .ok()
                .and_then(|plan| index_advice(&conn, &engine_for_advice, &sql_for_advice, plan));
            // The tree is a second EXPLAIN, in the engine's structured form.
            let plan = res
                .is_ok()
                .then(|| e_db::explain_tree(&conn, &sql_for_advice).ok().flatten())
                .flatten();
            send((res, advice, plan))
        });
    }

    /// EXPLAIN a query observed in the runtime panel against the panel's
    /// active database and show its plan in the DB panel (DB-604).
    pub fn db_explain_from_runtime(&self, sql: String) {
        let Some(entry) = self.db_active_entry("EXPLAIN") else {
            return;
        };
        self.db_explain_sql(entry, sql);
    }

    /// Run EXPLAIN on the SQL under the cursor and, if it scans where an
    /// index would help, add that index: as a migration in a Laravel
    /// project (DB-603). When no single index fits, ask the agent.
    pub fn suggest_index_under_cursor(&self) {
        let Some((entry, sql)) = self.sql_and_conn_under_cursor() else {
            return;
        };
        let Some(conn) = entry.conn.get_untracked() else {
            return;
        };
        let engine = entry.config.engine.clone();
        let engine_for_plan = engine.clone();
        let sql_for_explain = sql.clone();
        let state = *self;
        let ask = create_ext_action(
            self.cx,
            move |(issues, advice): (Vec<String>, Option<e_laravel::query_advice::Advice>)| {
                match advice {
                    Some(advice) => state.db_add_index(&advice, &engine),
                    None if issues.is_empty() => {
                        Self::notify("EXPLAIN found no full scans — no index needed")
                    }
                    None => state.send_to_agent(&index_prompt(&sql, &issues)),
                }
            },
        );
        std::thread::spawn(move || {
            let Ok(plan) = e_db::explain(&conn, &sql_for_explain) else {
                ask((Vec::new(), None));
                return;
            };
            let issues = e_db::analyze_explain(&engine_for_plan, &plan);
            let advice = index_advice(&conn, &engine_for_plan, &sql_for_explain, &plan);
            ask((issues, advice));
        });
    }

    /// The EXPLAIN banner's button: add the index the shown plan asks for,
    /// or, when no single index fits, ask the agent.
    pub fn db_suggest_index_for_plan(&self) {
        let tab = self
            .db
            .active_tab
            .get_untracked()
            .and_then(|i| self.db.result_tabs.with_untracked(|t| t.get(i).cloned()));
        let Some(tab) = tab else {
            return;
        };
        if let Some(advice) = &tab.advice {
            self.db_add_index(advice, &self.db_result_engine());
            return;
        }
        if let Some(sql) = &tab.sql {
            if !tab.explain.is_empty() {
                self.send_to_agent(&index_prompt(sql, &tab.explain));
            }
        }
    }

    /// The banner's other way: ask the agent, even when the index is known.
    pub fn db_ask_agent_for_index(&self) {
        let tab = self
            .db
            .active_tab
            .get_untracked()
            .and_then(|i| self.db.result_tabs.with_untracked(|t| t.get(i).cloned()));
        if let Some((sql, issues)) = tab.and_then(|t| Some((t.sql?, t.explain))) {
            self.send_to_agent(&index_prompt(&sql, &issues));
        }
    }

    /// Add `advice`'s index: a migration, opened, in a Laravel project (so
    /// it goes through version control like any change); elsewhere its
    /// `CREATE INDEX`, for `engine`, on the clipboard.
    pub(crate) fn db_add_index(&self, advice: &e_laravel::query_advice::Advice, engine: &str) {
        let root = self.root.get_untracked();
        if root.join("artisan").is_file() {
            self.write_index_migration(advice);
        } else {
            let _ = floem::Clipboard::set_contents(create_index_sql(engine, advice));
            Self::notify(&format!(
                "Not a Laravel project: CREATE INDEX on {} copied to the clipboard",
                advice.describe()
            ));
        }
    }

    fn db_apply_result(&self, res: Result<e_db::QueryResult, String>) {
        self.db.result_loading.set(false);
        self.db.selected_cell.set(None);
        // The table is shown in its own tab; result tabs stay beside it.
        self.db.active_tab.set(None);
        if let (Some(key), Some(table)) = (
            self.db.result_key.get_untracked(),
            self.db.result_table.get_untracked(),
        ) {
            self.db.browsed.set(Some((key, table)));
        }
        self.db.explain_issues.set(Vec::new());
        // Staged changes are keyed by primary key, so they outlive the rows
        // they were made on; place them on the new ones.
        self.db.pending_view.set(Default::default());
        match res {
            Ok(r) => {
                self.db.result_error.set(None);
                self.db.result.set(Some(r));
                self.db_load_pending_view();
            }
            Err(e) => {
                self.db.result.set(None);
                self.db.result_error.set(Some(e));
            }
        }
    }

    pub fn close_db_result(&self) {
        self.db.result_open.set(false);
        self.db.edit.set(None);
    }

    /// The user approved an agent-proposed query: run it and reply.
    pub fn db_consent_allow(&self) {
        let Some(c) = self.db.consent.get_untracked() else {
            return;
        };
        self.db.consent.set(None);
        std::thread::spawn(move || {
            let resp = match e_db::query(&c.conn, &c.sql, e_db::MAX_ROWS) {
                Ok(r) => serde_json::json!({
                    "ok": true,
                    "columns": r.columns,
                    "rows": r.rows,
                    "rows_affected": r.rows_affected,
                    "elapsed_ms": r.elapsed_ms,
                    "truncated": r.truncated,
                }),
                Err(e) => serde_json::json!({"ok": false, "error": e}),
            };
            let _ = c.reply.send(resp);
        });
    }

    /// The user rejected an agent-proposed query.
    pub fn db_consent_deny(&self) {
        if let Some(c) = self.db.consent.get_untracked() {
            self.db.consent.set(None);
            let _ = c
                .reply
                .send(serde_json::json!({"ok": false, "error": "denied by user"}));
        }
    }

    /// Whether the current results grid supports inline editing (a browsed table
    /// in data view, with a known primary key).
    pub fn db_editable(&self) -> bool {
        self.db.result_table.get_untracked().is_some()
            && self.db.subview.get_untracked() == "data"
            && self
                .db
                .columns
                .with_untracked(|c| c.iter().any(|c| c.key == "PRI"))
    }

    /// Begin editing the cell at `(row, col)`.
    pub fn db_begin_edit(&self, row: usize, col: usize) {
        if !self.db_editable() {
            return;
        }
        let Some(result) = self.db.result.get_untracked() else {
            return;
        };
        let Some(cell) = result.rows.get(row).and_then(|r| r.get(col)) else {
            return;
        };
        let column = result.columns.get(col).cloned().unwrap_or_default();
        // A one-line value of a JSON column opens laid out (DB-304).
        let is_json_col = self.db.columns.with_untracked(|cols| {
            cols.iter()
                .any(|c| c.name == column && c.data_type.to_ascii_lowercase().contains("json"))
        });
        let laid_out = cell
            .as_deref()
            .filter(|v| is_json_col && !v.contains('\n'))
            .and_then(crate::json_layout::pretty);
        self.db.edit_json.set(laid_out.as_ref().and(cell.clone()));
        self.db.edit_null.set(cell.is_none());
        self.db
            .edit_value
            .set(laid_out.or_else(|| cell.clone()).unwrap_or_default());
        self.db.edit.set(Some((row, col, column)));
    }

    pub fn db_cancel_edit(&self) {
        self.db.edit.set(None);
    }

    /// Toggle write protection for a connection (production defaults to on).
    pub fn db_toggle_read_only(&self, entry: DbEntry) {
        let now = !entry.read_only.get_untracked();
        entry.read_only.set(now);
        if let Some(conn) = entry.conn.get_untracked() {
            conn.set_read_only(now);
        }
        Self::notify(if now {
            "Database set to read-only"
        } else {
            "Database writes enabled"
        });
    }

    /// Stage the edited cell: show the new value in the grid and keep it
    /// until Submit writes it or Revert drops it.
    pub fn db_commit_edit(&self) {
        let Some((row, col, column)) = self.db.edit.get_untracked() else {
            return;
        };
        let Some((entry, _, table, _)) = self.db_edit_target() else {
            return;
        };
        // Write guard: refuse edits to a read-only (e.g. production) connection.
        if entry.read_only.get_untracked() {
            Self::notify(
                "Read-only: this connection is protected from writes (looks like production). \
                 Toggle read-only off in the Database panel to edit.",
            );
            self.db.edit.set(None);
            return;
        }
        let Some(pk) = self.db_shown_row_pk(row) else {
            Self::notify("Edit: table has no primary key — cannot edit safely");
            self.db.edit.set(None);
            return;
        };
        if !self.db_claim_pending_target(&entry.key(), &table) {
            self.db.edit.set(None);
            return;
        }
        let is_null = self.db.edit_null.get_untracked();
        let value = self.db.edit_value.get_untracked();
        // A JSON column takes only JSON: say what's wrong and keep the
        // editor open, rather than let the database refuse the whole Submit.
        let data_type = self.db.columns.with_untracked(|cols| {
            cols.iter()
                .find(|c| c.name == column)
                .map(|c| c.data_type.clone())
                .unwrap_or_default()
        });
        if !is_null {
            if let Some(problem) = json_problem(&data_type, &value) {
                Self::notify(&format!("{column} is {data_type}: {problem}"));
                return;
            }
        }
        let value = match self.db.edit_json.get_untracked() {
            Some(read) if !is_null => json_saved(&read, value),
            _ => value,
        };
        let new = if is_null { None } else { Some(value) };
        let shown = self.db.result.with_untracked(|r| {
            r.as_ref()
                .and_then(|r| r.rows.get(row))
                .and_then(|r| r.get(col))
                .cloned()
                .flatten()
        });

        // Stage the edit (transactional): reflect it in the grid and record it as
        // pending; the write happens on Submit.
        self.db.result.update(|r| {
            if let Some(r) = r {
                if let Some(cell) = r.rows.get_mut(row).and_then(|row| row.get_mut(col)) {
                    *cell = new.clone();
                }
            }
        });
        self.db.pending_edits.update(|m| {
            let k = (pk.clone(), column.clone());
            // Editing a cell again keeps the value it had in the database, and
            // setting it back to that value is no change at all.
            let old = m.get(&k).map(|e| e.old.clone()).unwrap_or(shown);
            if old == new {
                m.remove(&k);
            } else {
                m.insert(
                    k,
                    crate::state::PendingEdit {
                        column: column.clone(),
                        pk,
                        new,
                        old,
                    },
                );
            }
        });
        self.db_release_pending_target_if_empty();
        self.db_mark_pending();
        self.db.edit.set(None);
    }

    /// Primary-key `(name, value)` pairs for `row`, using the result's columns.
    fn db_row_pk_full(
        &self,
        row: usize,
        result: &e_db::QueryResult,
    ) -> Option<Vec<(String, Option<String>)>> {
        let pk_names: Vec<String> = self.db.columns.with_untracked(|cols| {
            cols.iter()
                .filter(|c| c.key == "PRI")
                .map(|c| c.name.clone())
                .collect()
        });
        if pk_names.is_empty() {
            return None;
        }
        let mut pk = Vec::new();
        for name in &pk_names {
            if let Some(idx) = result.columns.iter().position(|c| c == name) {
                pk.push((name.clone(), result.rows[row].get(idx).cloned().flatten()));
            }
        }
        Some(pk)
    }

    /// Resolve the edit overlay's connection + table + entry, honouring the
    /// read-only guard. Shared by row delete / FK-hop.
    fn db_edit_target(&self) -> Option<(DbEntry, Arc<e_db::Conn>, String, String)> {
        let (Some(key), Some(table)) = (
            self.db.result_key.get_untracked(),
            self.db.result_table.get_untracked(),
        ) else {
            return None;
        };
        let entry = self
            .db
            .conns
            .with_untracked(|c| c.iter().find(|e| e.key() == key).cloned())?;
        let conn = entry.conn.get_untracked()?;
        let engine = entry.config.engine.clone();
        Some((entry, conn, table, engine))
    }

    /// The primary key of shown row `row`, as read from the database.
    fn db_shown_row_pk(&self, row: usize) -> Option<RowPk> {
        self.db
            .pending_view
            .with_untracked(|v| v.row_pks.get(row).cloned().flatten())
    }

    /// Whether the grid shows the table the staged changes belong to.
    fn db_showing_pending_target(&self) -> bool {
        let (Some(key), Some(table)) = (
            self.db.result_key.get_untracked(),
            self.db.result_table.get_untracked(),
        ) else {
            return false;
        };
        self.db.pending_target.get_untracked() == Some((key, table))
    }

    /// Make `(key, table)` the table that staged changes belong to, unless
    /// another table already has some: one Submit is one transaction on one
    /// connection, so those must be submitted or reverted first.
    fn db_claim_pending_target(&self, key: &str, table: &str) -> bool {
        match self.db.pending_target.get_untracked() {
            Some((k, t)) if k == key && t == table => true,
            Some((_, t)) => {
                let n = self.db_pending_count();
                Self::notify(&format!(
                    "{n} change{} to {t} {} not submitted — Submit or Revert {} first",
                    if n == 1 { "" } else { "s" },
                    if n == 1 { "is" } else { "are" },
                    if n == 1 { "it" } else { "them" },
                ));
                false
            }
            None => {
                self.db
                    .pending_target
                    .set(Some((key.to_string(), table.to_string())));
                true
            }
        }
    }

    fn db_pending_count(&self) -> usize {
        self.db.pending_edits.with_untracked(|m| m.len())
            + self.db.pending_deletes.with_untracked(|m| m.len())
            + self.db.pending_inserts.with_untracked(|v| v.len())
    }

    fn db_release_pending_target_if_empty(&self) {
        if self.db_pending_count() == 0 {
            self.db.pending_target.set(None);
        }
    }

    /// Read the primary key of each shown row, once per result, and put the
    /// staged edits for the shown table into its cells.
    pub(crate) fn db_load_pending_view(&self) {
        let have = self
            .db
            .pending_view
            .with_untracked(|v| !v.row_pks.is_empty());
        if !have && self.db.result_table.get_untracked().is_some() {
            let pk_names: Vec<String> = self.db.columns.with_untracked(|cols| {
                cols.iter()
                    .filter(|c| c.key == "PRI")
                    .map(|c| c.name.clone())
                    .collect()
            });
            let row_pks = self
                .db
                .result
                .with_untracked(|r| r.as_ref().map(|r| row_pks(r, &pk_names)))
                .unwrap_or_default();
            if row_pks.iter().any(Option::is_some) {
                if self.db_showing_pending_target() {
                    let edits = self.db.pending_edits.get_untracked();
                    self.db.result.update(|r| {
                        if let Some(r) = r {
                            overlay_edits(r, &row_pks, &edits);
                        }
                    });
                }
                self.db.pending_view.update(|v| v.row_pks = row_pks);
            }
        }
        self.db_mark_pending();
    }

    /// Mark the shown cells and rows that have a staged change.
    fn db_mark_pending(&self) {
        let showing = self.db_showing_pending_target();
        let columns = self
            .db
            .result
            .with_untracked(|r| r.as_ref().map(|r| r.columns.clone()).unwrap_or_default());
        let edits = self.db.pending_edits.get_untracked();
        let deletes = self.db.pending_deletes.get_untracked();
        self.db.pending_view.update(|v| {
            v.cells.clear();
            v.rows.clear();
            if showing {
                mark_pending(v, &columns, &edits, &deletes);
            }
        });
    }

    /// Stage deleting the row open in the edit overlay; again un-stages it.
    pub fn db_delete_row(&self) {
        let Some((row, _, _)) = self.db.edit.get_untracked() else {
            return;
        };
        let Some((entry, _, table, _)) = self.db_edit_target() else {
            return;
        };
        if entry.read_only.get_untracked() {
            Self::notify(
                "Read-only: this connection is protected from writes (looks like production). \
                 Toggle read-only off in the Database panel to delete.",
            );
            self.db.edit.set(None);
            return;
        }
        let Some(pk) = self.db_shown_row_pk(row) else {
            Self::notify("Delete: table has no primary key — cannot delete safely");
            return;
        };
        if !self.db_claim_pending_target(&entry.key(), &table) {
            self.db.edit.set(None);
            return;
        }
        // Stage the deletion (transactional): mark the row pending; the DELETE
        // runs on Submit. Toggling again un-marks it. The row as the database
        // has it is kept, for the write log's Undo.
        let staged = self.db.pending_deletes.try_update(|m| {
            if m.remove(&pk) {
                false
            } else {
                m.insert(pk.clone());
                true
            }
        });
        if staged == Some(true) {
            let row_values = self.db.result.with_untracked(|r| {
                let r = r.as_ref()?;
                let shown = r.rows.get(row)?;
                self.db.columns.with_untracked(|cols| {
                    self.db.pending_edits.with_untracked(|edits| {
                        Some(row_for_undo(cols, &r.columns, shown, &pk, edits))
                    })
                })
            });
            self.db
                .deleted_rows
                .update(|m| _ = m.insert(pk, row_values.flatten()));
        } else {
            self.db.deleted_rows.update(|m| _ = m.remove(&pk));
        }
        self.db_release_pending_target_if_empty();
        self.db_mark_pending();
        self.db.edit.set(None);
    }

    /// Drop all staged changes, and show the table's real values again.
    pub fn db_revert_changes(&self) {
        self.db.conflicts.set(Vec::new());
        let showing = self.db_showing_pending_target();
        self.db_clear_pending();
        if showing {
            self.db_reload_table();
        }
    }

    fn db_clear_pending(&self) {
        self.db.pending_edits.update(|m| m.clear());
        self.db.pending_deletes.update(|m| m.clear());
        self.db.deleted_rows.update(|m| m.clear());
        self.db.pending_inserts.update(|v| v.clear());
        self.db.pending_target.set(None);
        self.db_mark_pending();
    }

    /// Review staged changes and (after confirmation) run them in one
    /// transaction. Builds UPDATE/DELETE statements from the pending sets.
    pub fn db_submit_changes(&self) {
        let Some((key, table)) = self.db.pending_target.get_untracked() else {
            return;
        };
        let Some(entry) = self
            .db
            .conns
            .with_untracked(|c| c.iter().find(|e| e.key() == key).cloned())
        else {
            Self::notify("Submit: the connection these changes belong to is gone");
            return;
        };
        if entry.conn.get_untracked().is_none() {
            Self::notify(&format!(
                "Submit: connect to {} first",
                entry.config.display_name()
            ));
            return;
        }
        if entry.read_only.get_untracked() {
            Self::notify(
                "Read-only: this connection is protected from writes. Toggle read-only off \
                 in the Database panel to submit.",
            );
            return;
        }
        let engine = entry.config.engine.clone();
        let mut stmts: Vec<String> = Vec::new();
        let mut log: Vec<crate::state::WriteLogEntry> = Vec::new();
        // Each edit and delete is checked against the row as it was read,
        // so a change made meanwhile isn't overwritten.
        let mut checks: Vec<(e_db::RowCheck, crate::state::Staged)> = Vec::new();
        // A row's edits go in one statement, so a change to its key and to
        // another column can't miss each other; a row also staged for
        // deletion is deleted, not edited first.
        let deleting = self.db.pending_deletes.get_untracked();
        self.db.pending_edits.with_untracked(|edits| {
            for (pk, row) in edits_by_row(edits) {
                if deleting.contains(&pk) {
                    continue;
                }
                let news: Vec<(String, Option<String>)> = row
                    .iter()
                    .map(|e| (e.column.clone(), e.new.clone()))
                    .collect();
                let olds: Vec<(String, Option<String>)> = row
                    .iter()
                    .map(|e| (e.column.clone(), e.old.clone()))
                    .collect();
                checks.push((
                    e_db::RowCheck {
                        table: table.clone(),
                        pk: pk.clone(),
                        columns: olds.clone(),
                    },
                    crate::state::Staged::Edits(
                        pk.clone(),
                        row.iter().map(|e| e.column.clone()).collect(),
                    ),
                ));
                let fwd = e_db::update_row_sql(&engine, &table, &news, &pk);
                // Reverse: the old values back, on the row by its key as it
                // is after the edit.
                let rev = e_db::update_row_sql(&engine, &table, &olds, &key_after(&pk, &news));
                stmts.push(fwd.clone());
                log.push(crate::state::WriteLogEntry {
                    key: key.clone(),
                    forward: fwd,
                    reverse: Some(rev),
                });
            }
        });
        self.db.pending_deletes.with_untracked(|dels| {
            for pk in dels {
                // The row must still be as it was read, not merely there:
                // a row someone changed meanwhile isn't the one chosen.
                let as_read = self
                    .db
                    .deleted_rows
                    .with_untracked(|m| m.get(pk).cloned().flatten())
                    .unwrap_or_default();
                checks.push((
                    e_db::RowCheck {
                        table: table.clone(),
                        pk: pk.clone(),
                        columns: as_read,
                    },
                    crate::state::Staged::Delete(pk.clone()),
                ));
                let fwd = e_db::delete_sql(&engine, &table, pk);
                stmts.push(fwd.clone());
                // Its reverse puts the row back as it was read.
                let reverse = self.db.deleted_rows.with_untracked(|m| {
                    m.get(pk)
                        .cloned()
                        .flatten()
                        .map(|values| e_db::insert_sql(&engine, &table, &values))
                });
                log.push(crate::state::WriteLogEntry {
                    key: key.clone(),
                    forward: fwd,
                    reverse,
                });
            }
        });
        self.db.pending_inserts.with_untracked(|rows| {
            for row in rows {
                stmts.push(row.sql.clone());
                log.push(crate::state::WriteLogEntry {
                    key: key.clone(),
                    forward: row.sql.clone(),
                    reverse: row.reverse.clone(),
                });
            }
        });
        if stmts.is_empty() {
            return;
        }
        let env = entry.config.environment();
        self.db_ask_confirm(crate::state::DbConfirm {
            verb: "Submit".into(),
            statements: stmts.clone(),
            env,
            needs_ack: !env.is_local(),
            ack: self.cx.create_rw_signal(false),
            run: crate::state::ConfirmRun::Transaction(crate::state::DbTransaction {
                key,
                statements: stmts,
                log,
                submits_pending: true,
                checks,
                done: "Submitted",
            }),
            counts: self.cx.create_rw_signal(Vec::new()),
        });
    }

    /// Open the confirmation dialog for `c`, and count the rows each of
    /// its statements will touch while it is open (DB-703). What a
    /// statement says itself (`INSERT … VALUES`) is shown at once; the
    /// rest are counted on the database, off the UI thread, for the first
    /// `MAX_COUNTED` statements.
    fn db_ask_confirm(&self, c: crate::state::DbConfirm) {
        use crate::state::RowEstimate;
        const MAX_COUNTED: usize = 50;
        let key = match &c.run {
            crate::state::ConfirmRun::Transaction(tx) => Some(tx.key.clone()),
            crate::state::ConfirmRun::Console(_) => self.db.result_key.get_untracked(),
        };
        let entry = key.and_then(|k| {
            self.db
                .conns
                .with_untracked(|cs| cs.iter().find(|e| e.key() == k).cloned())
        });
        let engine = entry
            .as_ref()
            .map(|e| e.config.engine.clone())
            .unwrap_or_default();
        let conn = entry.and_then(|e| e.conn.get_untracked());
        let mut queries = Vec::new();
        let estimates: Vec<RowEstimate> = c
            .statements
            .iter()
            .enumerate()
            .map(|(i, stmt)| match e_db::affects(&engine, stmt) {
                e_db::Affects::Rows(n) => RowEstimate::Known(e_db::Counted::from_count(n)),
                e_db::Affects::Count(_) if conn.is_some() && queries.len() < MAX_COUNTED => {
                    queries.push((i, stmt.clone()));
                    RowEstimate::Counting
                }
                _ => RowEstimate::Unknown,
            })
            .collect();
        let counts = c.counts;
        counts.set(estimates);
        self.db.confirm.set(Some(c));
        let Some(conn) = conn.filter(|_| !queries.is_empty()) else {
            return;
        };
        let send = create_ext_action(
            self.cx,
            move |found: Vec<(usize, Option<e_db::Counted>)>| {
                counts.update(|v| {
                    for (i, n) in found {
                        if let Some(slot) = v.get_mut(i) {
                            *slot = n.map_or(RowEstimate::Unknown, RowEstimate::Known);
                        }
                    }
                });
            },
        );
        std::thread::spawn(move || {
            let found = queries
                .into_iter()
                .map(|(i, stmt)| (i, e_db::count_affected(&conn, &engine, &stmt)))
                .collect();
            send(found);
        });
    }

    /// Run confirmed statements as one transaction, log them, and refresh the
    /// table if it is the one they wrote to.
    fn execute_submit(&self, tx: crate::state::DbTransaction) {
        let Some(entry) = self
            .db
            .conns
            .with_untracked(|c| c.iter().find(|e| e.key() == tx.key).cloned())
        else {
            return;
        };
        let Some(conn) = entry.conn.get_untracked() else {
            Self::notify(&format!("Connect to {} first", entry.config.display_name()));
            return;
        };
        self.db.result_loading.set(true);
        let state = *self;
        let crate::state::DbTransaction {
            key,
            statements,
            log,
            submits_pending,
            checks,
            done,
        } = tx;
        let (row_checks, staged): (Vec<_>, Vec<_>) = checks.into_iter().unzip();
        let checked = row_checks.clone();
        self.db.conflicts.set(Vec::new());
        let send = create_ext_action(self.cx, move |res: Result<u64, e_db::TxError>| {
            state.db.result_loading.set(false);
            match res {
                Ok(n) => {
                    if submits_pending {
                        state.db_clear_pending();
                    }
                    // Record the executed writes in the session undo-log.
                    state.db.write_log.update(|l| l.extend(log));
                    Self::notify(&format!("{done} — {n} row(s) affected"));
                    if state.db.result_key.get_untracked().as_deref() == Some(key.as_str())
                        && state.db.result_table.get_untracked().is_some()
                    {
                        state.db_reload_table();
                    }
                }
                Err(e_db::TxError::Conflicts(found)) => {
                    state.db_take_conflicts(&key, &checked, &staged, &found)
                }
                Err(e_db::TxError::Failed(e)) => state.db.result_error.set(Some(e)),
            }
        });
        std::thread::spawn(move || {
            send(e_db::execute_checked(&conn, &row_checks, &statements));
        });
    }

    /// After a Submit that found rows changed since they were read: take
    /// the staged changes they guard off (the rest stay staged), say what
    /// each row holds now, and read the table again so it shows that.
    fn db_take_conflicts(
        &self,
        key: &str,
        checks: &[e_db::RowCheck],
        staged: &[crate::state::Staged],
        found: &[e_db::Conflict],
    ) {
        use crate::state::Staged;
        let edits = self.db.pending_edits.get_untracked();
        let lines = found
            .iter()
            .filter_map(|c| {
                let check = checks.get(c.check)?;
                let staged = staged.get(c.check)?;
                Some(conflict_line(check, staged, &edits, &c.found))
            })
            .collect::<Vec<_>>();
        for c in found {
            match staged.get(c.check) {
                Some(Staged::Edits(pk, cols)) => {
                    // The edits of what changed, or of a row that's gone; the
                    // rest of the row's stay staged.
                    let taken: Vec<String> = match &c.found {
                        e_db::Found::Changed(changed) => {
                            changed.iter().map(|(col, _)| col.clone()).collect()
                        }
                        e_db::Found::Gone => cols.clone(),
                    };
                    self.db.pending_edits.update(|m| {
                        for col in taken {
                            m.remove(&(pk.clone(), col));
                        }
                    });
                }
                Some(Staged::Delete(pk)) => {
                    self.db.pending_deletes.update(|m| _ = m.remove(pk));
                    self.db.deleted_rows.update(|m| _ = m.remove(pk));
                }
                None => {}
            }
        }
        self.db_release_pending_target_if_empty();
        self.db_mark_pending();
        self.db.conflicts.set(lines);
        if self.db.result_key.get_untracked().as_deref() == Some(key)
            && self.db.result_table.get_untracked().is_some()
        {
            self.db_reload_table();
        }
    }

    /// Show rows in other tables that reference the current row (reverse FK).
    /// Uses the current table's primary key value; opens the first referencing
    /// table filtered to it.
    pub fn db_show_related(&self) {
        let Some((row, _, _)) = self.db.edit.get_untracked() else {
            return;
        };
        let (Some(key), Some(table)) = (
            self.db.result_key.get_untracked(),
            self.db.result_table.get_untracked(),
        ) else {
            return;
        };
        let Some(entry) = self
            .db
            .conns
            .with_untracked(|c| c.iter().find(|e| e.key() == key).cloned())
        else {
            return;
        };
        let Some(conn) = entry.conn.get_untracked() else {
            return;
        };
        let Some(result) = self.db.result.get_untracked() else {
            return;
        };
        let Some(pk) = self.db_row_pk_full(row, &result) else {
            Self::notify("Related: table has no primary key");
            return;
        };
        let Some((pk_col, Some(pk_val))) = pk.into_iter().next() else {
            return;
        };
        let pk_val_for_count = pk_val.clone();
        let engine = entry.config.engine.clone();
        let row_label = format!("{table} {pk_col}={pk_val}");
        let state = *self;
        // Each table that points here, with how many of its rows do.
        let send =
            create_ext_action(
                self.cx,
                move |refs: Vec<(String, String, Option<u64>)>| match refs.as_slice() {
                    [] => Self::notify("No tables reference this row"),
                    [(child, col, _)] => {
                        state.db_open_related(&key, child.clone(), col.clone(), pk_val.clone())
                    }
                    _ => {
                        state.db.edit.set(None);
                        state.db.related.set(Some(crate::state::RelatedChoice {
                            key: key.clone(),
                            row: row_label.clone(),
                            value: pk_val.clone(),
                            refs,
                        }));
                    }
                },
            );
        std::thread::spawn(move || {
            let refs = e_db::referencing(&conn, &table, &pk_col)
                .unwrap_or_default()
                .into_iter()
                .map(|(child, col)| {
                    let sql = format!(
                        "SELECT COUNT(*) FROM {} WHERE {} = {}",
                        e_db::quote_ident(&engine, &child),
                        e_db::quote_ident(&engine, &col),
                        e_db::quote_literal(&engine, &pk_val_for_count)
                    );
                    let n = e_db::query(&conn, &sql, 1)
                        .ok()
                        .and_then(|r| r.rows.into_iter().next())
                        .and_then(|r| r.into_iter().next().flatten())
                        .and_then(|n| n.trim().parse().ok());
                    (child, col, n)
                })
                .collect();
            send(refs);
        });
    }

    /// Open `table` on connection `key` filtered to the rows whose `column`
    /// is `value`: the rows that point at the one Related was asked from.
    pub fn db_open_related(&self, key: &str, table: String, column: String, value: String) {
        let Some(entry) = self
            .db
            .conns
            .with_untracked(|c| c.iter().find(|e| e.key() == key).cloned())
        else {
            return;
        };
        self.db.edit.set(None);
        self.db.related.set(None);
        self.db.result_key.set(Some(key.to_string()));
        self.db.result_table.set(Some(table.clone()));
        self.db
            .result_title
            .set(format!("{} · {table}", entry.config.display_name()));
        self.db.subview.set("data".into());
        self.db.sort.set(None);
        self.db.filter.set(Some((column, Some(value))));
        self.db.page.set(0);
        self.db.total_rows.set(None);
        self.db.columns.set(Vec::new());
        self.db.ddl.set(None);
        self.db_load_columns(entry, table);
        self.db_reload_table();
    }

    /// Hop to the foreign-key target of the column open in the edit overlay,
    /// filtered to the current cell's value.
    pub fn db_hop_fk(&self) {
        let Some((row, col, column)) = self.db.edit.get_untracked() else {
            return;
        };
        let Some((entry, conn, table, _)) = self.db_edit_target() else {
            return;
        };
        let value = self.db.result.with_untracked(|r| {
            r.as_ref().and_then(|r| {
                r.rows
                    .get(row)
                    .and_then(|row| row.get(col))
                    .cloned()
                    .flatten()
            })
        });
        let disp = entry.config.display_name();
        let key = entry.key();
        let state = *self;
        // Anything opened meanwhile wins over the hop when it comes back.
        let gen = self.db.browse_gen.get_untracked();
        // `entry` holds non-Send signals, so it stays on the UI thread (in this
        // closure); only the target's name crosses the channel.
        let send = create_ext_action(self.cx, move |out: Option<(String, String)>| match out {
            None => Self::notify("No foreign key on this column"),
            Some(_) if state.db.browse_gen.get_untracked() != gen => {}
            Some((ref_table, ref_col)) => {
                state.db.edit.set(None);
                state.db.result_key.set(Some(key.clone()));
                state.db.result_table.set(Some(ref_table.clone()));
                state.db.result_title.set(format!("{disp} · {ref_table}"));
                state.db.subview.set("data".into());
                state.db.sort.set(None);
                // The referenced row, as a filter the toolbar shows and
                // paging keeps.
                state.db.filter.set(Some((ref_col, value.clone())));
                state.db.page.set(0);
                state.db.total_rows.set(None);
                state.db.columns.set(Vec::new());
                state.db.ddl.set(None);
                state.db_load_columns(entry.clone(), ref_table);
                state.db_reload_table();
            }
        });
        std::thread::spawn(move || {
            send(e_db::fk_target(&conn, &table, &column).ok().flatten());
        });
    }
}

type PendingEdits = HashMap<(RowPk, String), crate::state::PendingEdit>;

/// The primary key of each row of `r`, from the `pk` columns; `None` where
/// the result lacks one of them.
fn row_pks(r: &e_db::QueryResult, pk: &[String]) -> Vec<Option<RowPk>> {
    let idx: Option<Vec<usize>> = pk
        .iter()
        .map(|n| r.columns.iter().position(|c| c == n))
        .collect();
    r.rows
        .iter()
        .map(|row| {
            let idx = idx.as_ref().filter(|i| !i.is_empty())?;
            Some(
                pk.iter()
                    .zip(idx)
                    .map(|(n, &i)| (n.clone(), row.get(i).cloned().flatten()))
                    .collect(),
            )
        })
        .collect()
}

/// Put the staged edits into the cells of `r` they belong to.
fn overlay_edits(r: &mut e_db::QueryResult, row_pks: &[Option<RowPk>], edits: &PendingEdits) {
    for (ri, pk) in row_pks.iter().enumerate() {
        let Some(pk) = pk else { continue };
        for (ci, col) in r.columns.iter().enumerate() {
            if let Some(e) = edits.get(&(pk.clone(), col.clone())) {
                if let Some(cell) = r.rows.get_mut(ri).and_then(|row| row.get_mut(ci)) {
                    *cell = e.new.clone();
                }
            }
        }
    }
}

/// Mark in `v` the cells with a staged edit and the rows staged for deletion.
fn mark_pending(
    v: &mut crate::state::PendingView,
    columns: &[String],
    edits: &PendingEdits,
    deletes: &HashSet<RowPk>,
) {
    for (ri, pk) in v.row_pks.iter().enumerate() {
        let Some(pk) = pk else { continue };
        if deletes.contains(pk) {
            v.rows.insert(ri);
        }
        for (ci, col) in columns.iter().enumerate() {
            if edits.contains_key(&(pk.clone(), col.clone())) {
                v.cells.insert((ri, ci));
            }
        }
    }
}

/// The primary key of a row about to be inserted with `values`, when every
/// key column is given a value.
fn insert_pk(pk: &[String], values: &[(String, Option<String>)]) -> Option<RowPk> {
    if pk.is_empty() {
        return None;
    }
    pk.iter()
        .map(|n| values.iter().find(|(c, v)| c == n && v.is_some()).cloned())
        .collect()
}

/// Whether two configurations are the same database: the engine, and the
/// file, or the server and the database on it.
fn same_database(a: &e_db::DbConfig, b: &e_db::DbConfig) -> bool {
    let family = |e: &str| match e {
        "mariadb" => "mysql".to_string(),
        "postgresql" | "pgsql" => "postgres".to_string(),
        other => other.to_string(),
    };
    if family(&a.engine) != family(&b.engine) {
        return false;
    }
    if family(&a.engine) == "sqlite" {
        let real = |p: &str| std::fs::canonicalize(p).unwrap_or_else(|_| p.into());
        return real(&a.path) == real(&b.path);
    }
    let host = |h: &str| match h {
        "localhost" | "127.0.0.1" | "::1" => "localhost".to_string(),
        other => other.to_ascii_lowercase(),
    };
    host(&a.host) == host(&b.host) && a.port == b.port && a.database == b.database
}

/// A row staged for deletion as the database has it, to insert again: the
/// shown values, with any staged edit's value put back to what was read.
/// `None` for a table with binary columns, whose values the grid shows only
/// as their size.
fn row_for_undo(
    columns: &[e_db::ColumnInfo],
    shown_columns: &[String],
    shown: &[Option<String>],
    pk: &RowPk,
    edits: &PendingEdits,
) -> Option<RowPk> {
    let binary = columns.iter().any(|c| {
        let t = c.data_type.to_ascii_lowercase();
        ["blob", "binary", "bytea", "image"]
            .iter()
            .any(|b| t.contains(b))
    });
    if binary {
        return None;
    }
    Some(
        shown_columns
            .iter()
            .zip(shown)
            .map(|(col, v)| {
                let v = match edits.get(&(pk.clone(), col.clone())) {
                    Some(edit) => edit.old.clone(),
                    None => v.clone(),
                };
                (col.clone(), v)
            })
            .collect(),
    )
}

/// How long ago `then` was, from `now` (both in seconds since the epoch).
pub(crate) fn ago(now: u64, then: u64) -> String {
    let s = now.saturating_sub(then);
    let plural = |n: u64, unit: &str| format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" });
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86_399 => plural(s / 3600, "hour"),
        86_400..=172_799 => "yesterday".into(),
        _ => plural(s / 86_400, "day"),
    }
}

/// A file's size, as `12 KB` or `4.2 MB`.
pub(crate) fn file_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64 / 1024.0;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if v < 10.0 {
        format!("{v:.1} {}", UNITS[unit])
    } else {
        format!("{v:.0} {}", UNITS[unit])
    }
}

/// A row's staged edits, by its key.
fn edits_by_row(edits: &PendingEdits) -> Vec<(RowPk, Vec<&crate::state::PendingEdit>)> {
    let mut rows: Vec<(RowPk, Vec<&crate::state::PendingEdit>)> = Vec::new();
    for e in edits.values() {
        match rows.iter_mut().find(|(pk, _)| *pk == e.pk) {
            Some((_, row)) => row.push(e),
            None => rows.push((e.pk.clone(), vec![e])),
        }
    }
    for (_, row) in &mut rows {
        row.sort_by(|a, b| a.column.cmp(&b.column));
    }
    rows
}

/// A row's key after `sets` are written: any key column set takes its new
/// value.
fn key_after(pk: &RowPk, sets: &[(String, Option<String>)]) -> RowPk {
    pk.iter()
        .map(|(col, v)| {
            let v = sets
                .iter()
                .find(|(c, _)| c == col)
                .map_or_else(|| v.clone(), |(_, new)| new.clone());
            (col.clone(), v)
        })
        .collect()
}

/// What a failed check found, as one line of the conflict banner, e.g.
/// `id=5: email is now “b@x” (you read “a@x”) — your edit is no longer
/// staged`, with the values of yours taken off.
fn conflict_line(
    check: &e_db::RowCheck,
    staged: &crate::state::Staged,
    edits: &PendingEdits,
    found: &e_db::Found,
) -> crate::state::ConflictLine {
    use crate::state::{ConflictLine, Staged};
    use e_db::Found;
    let row = check
        .pk
        .iter()
        .map(|(c, v)| format!("{c}={}", v.as_deref().unwrap_or("NULL")))
        .collect::<Vec<_>>()
        .join(", ");
    let read = |col: &str| {
        check
            .columns
            .iter()
            .find(|(c, _)| c == col)
            .and_then(|(_, v)| v.clone())
    };
    // `email is now “b@x” (you read “a@x”)`, and how many more changed.
    let what_changed = |changed: &[(String, Option<String>)]| {
        let (col, now) = &changed[0];
        let more = match changed.len() {
            1 => String::new(),
            2 => " and 1 more column".into(),
            n => format!(" and {} more columns", n - 1),
        };
        format!(
            "{col} is now {} (you read {}){more}",
            shown_value(now),
            shown_value(&read(col))
        )
    };
    let mine_of = |pk: &RowPk, cols: &[String]| -> Vec<(String, Option<String>)> {
        cols.iter()
            .filter_map(|c| {
                edits
                    .get(&(pk.clone(), c.clone()))
                    .map(|e| (c.clone(), e.new.clone()))
            })
            .collect()
    };
    match (staged, found) {
        (Staged::Edits(pk, cols), Found::Gone) => ConflictLine {
            text: format!("{row}: the row is gone — your edit is no longer staged"),
            mine: mine_of(pk, cols),
        },
        (Staged::Delete(_), Found::Gone) => ConflictLine {
            text: format!("{row}: the row is already gone — its delete is no longer staged"),
            mine: Vec::new(),
        },
        (Staged::Edits(pk, _), Found::Changed(changed)) => {
            let cols: Vec<String> = changed.iter().map(|(c, _)| c.clone()).collect();
            ConflictLine {
                text: format!(
                    "{row}: {} — your edit of it is no longer staged",
                    what_changed(changed)
                ),
                mine: mine_of(pk, &cols),
            }
        }
        (Staged::Delete(_), Found::Changed(changed)) => ConflictLine {
            text: format!(
                "{row}: {} — its delete is no longer staged",
                what_changed(changed)
            ),
            mine: Vec::new(),
        },
    }
}

/// Yours, to copy from the conflict banner: a value as it is, several as
/// `column: value` lines.
pub(crate) fn mine_to_copy(mine: &[(String, Option<String>)]) -> String {
    match mine {
        [(_, v)] => v.clone().unwrap_or_else(|| "NULL".into()),
        _ => mine
            .iter()
            .map(|(c, v)| format!("{c}: {}", v.as_deref().unwrap_or("NULL")))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// A value for a message: quoted and cut to a line, or `NULL`.
fn shown_value(v: &Option<String>) -> String {
    const MAX: usize = 40;
    match v {
        None => "NULL".into(),
        Some(s) => {
            let line = s.lines().next().unwrap_or("");
            let cut: String = line.chars().take(MAX).collect();
            if cut.len() < s.len() {
                format!("“{cut}…”")
            } else {
                format!("“{cut}”")
            }
        }
    }
}

/// What to save for a JSON cell that was `read` on one line and edited
/// laid out as `edited`: what was read if nothing changed, else `edited`
/// on one line again, spaced as `read` was.
fn json_saved(read: &str, edited: String) -> String {
    use crate::json_layout::{compact, pretty};
    if pretty(read).as_deref() == Some(edited.as_str()) {
        return read.to_string();
    }
    let spaced = compact(read, true) == read;
    compact(&edited, spaced)
}

/// Why `value` can't go in a column of `data_type`, when that is JSON and
/// the value isn't.
fn json_problem(data_type: &str, value: &str) -> Option<String> {
    if !data_type.to_ascii_lowercase().contains("json") {
        return None;
    }
    serde_json::from_str::<serde_json::Value>(value)
        .err()
        .map(|e| format!("not valid JSON ({e})"))
}

/// A plan, the index it asks for, and the plan as a tree.
type ExplainedPlan = (
    Result<e_db::QueryResult, String>,
    Option<e_laravel::query_advice::Advice>,
    Option<e_db::PlanNode>,
);

/// The index `plan` (of `sql`, on `engine`) asks for, unless the table
/// already has one that starts with its columns. Reads the table's indexes.
fn index_advice(
    conn: &e_db::Conn,
    engine: &str,
    sql: &str,
    plan: &e_db::QueryResult,
) -> Option<e_laravel::query_advice::Advice> {
    use e_laravel::query_advice::{index_for, verdict, Explained};
    let table = index_for(sql)?.table;
    let indexes = e_db::indexes(conn, &table)
        .unwrap_or_default()
        .into_iter()
        .map(|i| (table.clone(), i.columns))
        .collect();
    verdict(&[Explained {
        sql: sql.to_string(),
        engine: engine.to_string(),
        columns: plan.columns.clone(),
        rows: plan.rows.clone(),
        error: None,
        indexes,
    }])
    .advice
}

/// Whether an index is the one behind a primary key, which `CREATE INDEX`
/// doesn't make (MySQL's `PRIMARY`, PostgreSQL's `…_pkey`, SQLite's own).
pub(crate) fn is_key_index(name: &str) -> bool {
    name == "PRIMARY" || name.ends_with("_pkey") || name.starts_with("sqlite_autoindex_")
}

/// The `CREATE INDEX` that makes `ix` on `table`, quoted for `engine`.
fn index_ddl(engine: &str, table: &str, ix: &e_db::IndexInfo) -> String {
    let cols = ix
        .columns
        .iter()
        .map(|c| e_db::quote_ident(engine, c))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "CREATE {}INDEX {} ON {} ({cols});",
        if ix.unique { "UNIQUE " } else { "" },
        e_db::quote_ident(engine, &ix.name),
        e_db::quote_ident(engine, table)
    )
}

/// `CREATE INDEX` for `advice`, named the way Laravel names it.
fn create_index_sql(engine: &str, advice: &e_laravel::query_advice::Advice) -> String {
    let name = format!("{}_{}_index", advice.table, advice.columns.join("_"));
    let cols = advice
        .columns
        .iter()
        .map(|c| e_db::quote_ident(engine, c))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "CREATE INDEX {} ON {} ({cols});",
        e_db::quote_ident(engine, &name),
        e_db::quote_ident(engine, &advice.table)
    )
}

/// What the agent is asked to fix a slow query's plan with.
fn index_prompt(sql: &str, issues: &[String]) -> String {
    format!(
        "This SQL query has a performance problem. Propose a Laravel migration that adds the \
         missing index(es), then briefly explain why.\n\nQuery:\n{sql}\n\nEXPLAIN \
         findings:\n- {}",
        issues.join("\n- ")
    )
}

/// Which of the `connected` connections (by key) the panel means: the one
/// its results come from (`current`), else the only one there is.
fn pick_active(current: Option<&str>, connected: &[String]) -> Option<usize> {
    current
        .and_then(|k| connected.iter().position(|c| c == k))
        .or(if connected.len() == 1 { Some(0) } else { None })
}

/// The result tab shown after closing tab `closed`, leaving `len_after` tabs,
/// when `active` was shown (`None`: the browsed table, which stays).
fn active_after_close(active: Option<usize>, closed: usize, len_after: usize) -> Option<usize> {
    let a = active?;
    if a > closed {
        Some(a - 1)
    } else if a < closed {
        Some(a)
    } else if len_after == 0 {
        None
    } else {
        Some(closed.min(len_after - 1))
    }
}

#[cfg(test)]
mod tab_tests {
    use super::*;

    #[test]
    fn the_active_database_is_the_shown_one_or_the_only_one() {
        let two = vec!["a".to_string(), "b".to_string()];
        assert_eq!(pick_active(Some("b"), &two), Some(1));
        // Not the first connected one when there are several to choose from.
        assert_eq!(pick_active(None, &two), None);
        assert_eq!(pick_active(Some("gone"), &two), None);
        let one = vec!["a".to_string()];
        assert_eq!(pick_active(None, &one), Some(0));
        assert_eq!(pick_active(Some("gone"), &one), Some(0));
        assert_eq!(pick_active(Some("a"), &[]), None);
    }

    #[test]
    fn a_conflict_says_what_the_row_holds_now() {
        use crate::state::{PendingEdit, Staged};
        let pk = vec![("id".to_string(), Some("5".to_string()))];
        let check = e_db::RowCheck {
            table: "users".into(),
            pk: pk.clone(),
            columns: vec![
                ("email".into(), Some("a@x".into())),
                ("name".into(), Some("ada".into())),
            ],
        };
        let mut edits = PendingEdits::default();
        for (col, new, old) in [("email", "c@x", "a@x"), ("name", "Ada", "ada")] {
            edits.insert(
                (pk.clone(), col.into()),
                PendingEdit {
                    column: col.into(),
                    pk: pk.clone(),
                    new: Some(new.into()),
                    old: Some(old.into()),
                },
            );
        }
        let edit = Staged::Edits(pk.clone(), vec!["email".into(), "name".into()]);
        let line = conflict_line(
            &check,
            &edit,
            &edits,
            &e_db::Found::Changed(vec![("email".into(), None)]),
        );
        assert_eq!(
            line.text,
            "id=5: email is now NULL (you read “a@x”) — your edit of it is no longer staged"
        );
        // What you wrote is kept, to copy.
        assert_eq!(
            line.mine,
            vec![("email".to_string(), Some("c@x".to_string()))]
        );
        assert_eq!(mine_to_copy(&line.mine), "c@x");
        let gone = conflict_line(&check, &edit, &edits, &e_db::Found::Gone);
        assert_eq!(
            gone.text,
            "id=5: the row is gone — your edit is no longer staged"
        );
        assert_eq!(mine_to_copy(&gone.mine), "email: c@x\nname: Ada");
        let both = conflict_line(
            &check,
            &Staged::Delete(pk.clone()),
            &edits,
            &e_db::Found::Changed(vec![
                ("email".into(), Some("b@x".into())),
                ("name".into(), Some("bo".into())),
            ]),
        );
        assert_eq!(
            both.text,
            "id=5: email is now “b@x” (you read “a@x”) and 1 more column — its delete is \
             no longer staged"
        );
        assert_eq!(
            shown_value(&Some("x".repeat(50))),
            format!("“{}…”", "x".repeat(40))
        );
        assert_eq!(shown_value(&Some("one\ntwo".into())), "“one…”");
    }

    #[test]
    fn a_rows_edits_are_one_statement_and_its_undo_finds_it_by_its_new_key() {
        use crate::state::PendingEdit;
        let pk: RowPk = vec![("id".into(), Some("1".into()))];
        let mut edits = PendingEdits::default();
        for (col, new, old) in [("name", "Ada", "ada"), ("id", "100", "1")] {
            edits.insert(
                (pk.clone(), col.into()),
                PendingEdit {
                    column: col.into(),
                    pk: pk.clone(),
                    new: Some(new.into()),
                    old: Some(old.into()),
                },
            );
        }
        let rows = edits_by_row(&edits);
        assert_eq!(rows.len(), 1);
        let news: Vec<(String, Option<String>)> = rows[0]
            .1
            .iter()
            .map(|e| (e.column.clone(), e.new.clone()))
            .collect();
        assert_eq!(
            e_db::update_row_sql("sqlite", "users", &news, &pk),
            "UPDATE `users` SET `id` = '100', `name` = 'Ada' WHERE `id` = '1'"
        );
        assert_eq!(
            key_after(&pk, &news),
            vec![("id".to_string(), Some("100".to_string()))]
        );
    }

    #[test]
    fn a_json_cell_is_saved_on_one_line_as_it_was_read() {
        let read = r#"{"b": [1, 2], "a": "x"}"#;
        let shown = crate::json_layout::pretty(read).unwrap();
        // Unchanged: exactly what was read.
        assert_eq!(json_saved(read, shown.clone()), read);
        // Changed: one line again, in the same style, key order kept.
        let edited = shown.replace("\"x\"", "\"y\"");
        assert_eq!(json_saved(read, edited), r#"{"b": [1, 2], "a": "y"}"#);
        let tight = r#"{"b":1}"#;
        let edited = crate::json_layout::pretty(tight).unwrap().replace('1', "2");
        assert_eq!(json_saved(tight, edited), r#"{"b":2}"#);
    }

    #[test]
    fn indexes_and_advice_become_create_index() {
        let ix = e_db::IndexInfo {
            name: "orders_status_created_at_index".into(),
            unique: false,
            columns: vec!["status".into(), "created_at".into()],
        };
        assert_eq!(
            index_ddl("postgres", "orders", &ix),
            "CREATE INDEX \"orders_status_created_at_index\" ON \"orders\" (\"status\", \"created_at\");"
        );
        let unique = e_db::IndexInfo {
            unique: true,
            name: "users_email_unique".into(),
            columns: vec!["email".into()],
        };
        assert_eq!(
            index_ddl("mysql", "users", &unique),
            "CREATE UNIQUE INDEX `users_email_unique` ON `users` (`email`);"
        );
        assert!(is_key_index("PRIMARY") && is_key_index("users_pkey"));
        assert!(!is_key_index("users_email_unique"));
        let advice = e_laravel::query_advice::Advice {
            table: "orders".into(),
            columns: vec!["status".into(), "created_at".into()],
        };
        assert_eq!(
            create_index_sql("mysql", &advice),
            "CREATE INDEX `orders_status_created_at_index` ON `orders` (`status`, `created_at`);"
        );
    }

    #[test]
    fn a_deleted_row_comes_back_as_it_was_read() {
        use crate::state::PendingEdit;
        let col = |name: &str, ty: &str| e_db::ColumnInfo {
            name: name.into(),
            data_type: ty.into(),
            nullable: true,
            key: String::new(),
        };
        let cols = vec![
            col("id", "integer"),
            col("name", "text"),
            col("note", "text"),
        ];
        let shown_cols: Vec<String> = ["id", "name", "note"].map(String::from).to_vec();
        let pk: RowPk = vec![("id".into(), Some("1".into()))];
        // `name` shows a staged edit; the row goes back with what was read.
        let mut edits = PendingEdits::default();
        edits.insert(
            (pk.clone(), "name".into()),
            PendingEdit {
                column: "name".into(),
                pk: pk.clone(),
                new: Some("edited".into()),
                old: Some("ada".into()),
            },
        );
        let shown = vec![Some("1".into()), Some("edited".into()), None];
        assert_eq!(
            row_for_undo(&cols, &shown_cols, &shown, &pk, &edits),
            Some(vec![
                ("id".into(), Some("1".into())),
                ("name".into(), Some("ada".into())),
                ("note".into(), None),
            ])
        );
        let with_blob = vec![col("id", "integer"), col("avatar", "BLOB")];
        assert_eq!(
            row_for_undo(&with_blob, &shown_cols, &shown, &pk, &edits),
            None
        );
    }

    #[test]
    fn times_and_sizes_read_at_a_glance() {
        let now = 1_000_000;
        assert_eq!(ago(now, now - 5), "just now");
        assert_eq!(ago(now, now - 12 * 60), "12 min ago");
        assert_eq!(ago(now, now - 3600), "1 hour ago");
        assert_eq!(ago(now, now - 5 * 3600), "5 hours ago");
        assert_eq!(ago(now, now - 30 * 3600), "yesterday");
        assert_eq!(ago(now, now - 3 * 86_400), "3 days ago");
        assert_eq!(ago(now, now + 10), "just now");
        assert_eq!(file_size(512), "512 B");
        assert_eq!(file_size(12 * 1024), "12 KB");
        assert_eq!(file_size(4_400_000), "4.2 MB");
        assert_eq!(file_size(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn a_json_column_takes_only_json() {
        assert_eq!(json_problem("text", "{oops"), None);
        assert_eq!(json_problem("jsonb", "{\"a\": [1, 2]}"), None);
        assert_eq!(json_problem("JSON", "\"just a string\""), None);
        let p = json_problem("json", "{\"a\": }").unwrap();
        assert!(p.starts_with("not valid JSON"), "{p}");
        assert!(json_problem("jsonb", "").is_some());
    }

    #[test]
    fn closing_a_tab_keeps_the_shown_one_shown() {
        // The table stays shown whatever tab is closed.
        assert_eq!(active_after_close(None, 0, 2), None);
        // A tab before the shown one moves it left; one after leaves it.
        assert_eq!(active_after_close(Some(2), 0, 2), Some(1));
        assert_eq!(active_after_close(Some(0), 1, 1), Some(0));
        // Closing the shown tab shows the next one, or the one before at the end.
        assert_eq!(active_after_close(Some(1), 1, 2), Some(1));
        assert_eq!(active_after_close(Some(2), 2, 2), Some(1));
        // Closing the last tab goes back to the table.
        assert_eq!(active_after_close(Some(0), 0, 0), None);
    }
}

#[cfg(test)]
mod pending_tests {
    use super::*;
    use crate::state::{PendingEdit, PendingView};

    fn page(rows: &[(&str, &str)]) -> e_db::QueryResult {
        e_db::QueryResult {
            columns: vec!["id".into(), "name".into()],
            rows: rows
                .iter()
                .map(|(id, n)| vec![Some(id.to_string()), Some(n.to_string())])
                .collect(),
            ..Default::default()
        }
    }

    fn pk(id: &str) -> RowPk {
        vec![("id".into(), Some(id.into()))]
    }

    fn edit(id: &str, new: &str, old: &str) -> ((RowPk, String), PendingEdit) {
        (
            (pk(id), "name".into()),
            PendingEdit {
                column: "name".into(),
                pk: pk(id),
                new: Some(new.into()),
                old: Some(old.into()),
            },
        )
    }

    #[test]
    fn staged_changes_follow_their_rows_to_another_page() {
        // Edited on one page, row 2 shows up first on the next (sorted) page.
        let edits: PendingEdits = [edit("2", "Bobby", "Bob")].into_iter().collect();
        let deletes: HashSet<RowPk> = [pk("3")].into_iter().collect();
        let mut r = page(&[("2", "Bob"), ("9", "Zed"), ("3", "Cy")]);
        let pks = row_pks(&r, &["id".into()]);
        overlay_edits(&mut r, &pks, &edits);
        assert_eq!(r.rows[0][1].as_deref(), Some("Bobby"));
        assert_eq!(r.rows[1][1].as_deref(), Some("Zed"));

        let mut v = PendingView {
            row_pks: pks,
            ..Default::default()
        };
        mark_pending(&mut v, &r.columns, &edits, &deletes);
        assert_eq!(v.cells, [(0, 1)].into_iter().collect());
        assert_eq!(v.rows, [2].into_iter().collect());
    }

    #[test]
    fn a_page_without_the_staged_rows_shows_no_changes() {
        let edits: PendingEdits = [edit("2", "Bobby", "Bob")].into_iter().collect();
        let mut r = page(&[("7", "Ann")]);
        let pks = row_pks(&r, &["id".into()]);
        overlay_edits(&mut r, &pks, &edits);
        assert_eq!(r.rows[0][1].as_deref(), Some("Ann"));
        let mut v = PendingView {
            row_pks: pks,
            ..Default::default()
        };
        mark_pending(&mut v, &r.columns, &edits, &HashSet::new());
        assert!(v.cells.is_empty() && v.rows.is_empty());
    }

    #[test]
    fn rows_have_no_key_without_a_primary_key_or_its_column() {
        let r = page(&[("1", "Ann")]);
        assert_eq!(row_pks(&r, &[]), vec![None]);
        assert_eq!(row_pks(&r, &["uuid".into()]), vec![None]);
    }

    #[test]
    fn an_insert_has_a_key_only_when_every_key_column_is_given() {
        let pk_cols = vec!["id".to_string()];
        let with = vec![("id".into(), Some("5".into())), ("name".into(), None)];
        assert_eq!(insert_pk(&pk_cols, &with), Some(pk("5")));
        let auto = vec![("name".into(), Some("Ann".into()))];
        assert_eq!(insert_pk(&pk_cols, &auto), None);
        let null_id = vec![("id".into(), None)];
        assert_eq!(insert_pk(&pk_cols, &null_id), None);
        assert_eq!(insert_pk(&[], &with), None);
    }
}
