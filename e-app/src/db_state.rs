//! Database panel state: connections, table browsing, querying and inline cell
//! editing across MySQL, PostgreSQL, SQLite and ClickHouse.
//!
//! The view lives in [`crate::db_view`]; this module owns the `AppState` methods
//! that drive it. Extracted from the former `state.rs` god-module (fields stay on
//! `AppState`); same pattern as [`crate::debug`] / [`crate::runtime`].

use std::sync::Arc;

use floem::ext_event::create_ext_action;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::selection::Selection;
use floem::views::editor::text::Document;

use e_core::language::Language;

use crate::state::{AppState, DbEntry, DbForm, InsertField};

/// Rows per page when browsing a table in the Database panel.
const DB_PAGE: usize = 200;

/// Quote a CSV field if it contains a comma, quote or newline.
fn csv_escape(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

impl AppState {
    // ---- Database panel ------------------------------------------------

    pub fn toggle_db_panel(&self) {
        let open = !self.db_open.get_untracked();
        self.db_open.set(open);
        if open && self.db_conns.with_untracked(|c| c.is_empty()) {
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
        self.db_conns.set(entries);
        self.db_queries.set(e_db::load_queries(&root));
    }

    /// Fetch the project's DB schema (from `.env`) into an in-memory cache for
    /// Eloquent attribute completion. Runs in the background.
    pub fn load_db_schema_cache(&self) {
        let root = self.root.get_untracked();
        let Some(cfg) = e_db::from_env(&root) else {
            return;
        };
        let sig = self.db_schema_cache;
        let send = create_ext_action(
            self.cx,
            move |m: std::collections::HashMap<String, Vec<e_db::ColumnInfo>>| sig.set(m),
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

    /// Save the current query editor text under the typed name.
    pub fn db_save_query(&self) {
        let name = self.db_query_name.get_untracked().trim().to_string();
        let sql = self.db_query_text.get_untracked();
        if name.is_empty() || sql.trim().is_empty() {
            self.db_saving_query.set(false);
            return;
        }
        self.db_queries.update(|q| {
            q.retain(|x| x.name != name);
            q.push(e_db::SavedQuery { name, sql });
            q.sort_by(|a, b| a.name.cmp(&b.name));
        });
        let _ = e_db::save_queries(&self.root.get_untracked(), &self.db_queries.get_untracked());
        self.db_query_name.set(String::new());
        self.db_saving_query.set(false);
    }

    /// Load a saved query into the editor.
    pub fn db_load_query(&self, sql: String) {
        self.set_console_sql(sql);
    }

    /// Open the query-history panel and load the most recent entries.
    pub fn db_open_history(&self) {
        self.db_history_query.set(String::new());
        self.db_reload_history();
        self.db_history_open.set(true);
    }

    /// (Re)load history entries for the current project, honouring the search box.
    pub fn db_reload_history(&self) {
        let Some(path) = crate::config::history_db_path() else {
            return;
        };
        let project = self.root.get_untracked().to_string_lossy().into_owned();
        let needle = self.db_history_query.get_untracked();
        let entries = if needle.trim().is_empty() {
            e_db::history::recent(&path, &project, 200)
        } else {
            e_db::history::search(&path, &project, needle.trim(), 200)
        }
        .unwrap_or_default();
        self.db_history.set(entries);
    }

    /// Load a history entry's SQL back into the console and close the panel.
    pub fn db_reopen_history(&self, sql: String) {
        self.set_console_sql(sql);
        self.db_history_open.set(false);
    }

    /// Clear all query history for the current project.
    pub fn db_clear_history(&self) {
        if let Some(path) = crate::config::history_db_path() {
            let project = self.root.get_untracked().to_string_lossy().into_owned();
            let _ = e_db::history::clear(&path, &project);
        }
        self.db_history.set(Vec::new());
    }

    /// Set the SQL console text, keeping the `db_query_text` signal and the
    /// editor's document in sync. Programmatic callers (browse queries,
    /// run-under-cursor, saved/history queries) go through here so the editor
    /// reflects the change; the editor's own edits mirror back the other way.
    pub fn set_console_sql(&self, sql: String) {
        self.db_query_text.set(sql.clone());
        if let Some(doc) = self.db_console_doc.get_untracked() {
            if doc.text().to_string() != sql {
                let len = doc.text().len();
                doc.edit_single(Selection::region(0, len), &sql, EditType::InsertChars);
            }
        }
    }

    #[allow(dead_code)]
    pub fn db_delete_query(&self, name: String) {
        self.db_queries.update(|q| q.retain(|x| x.name != name));
        let _ = e_db::save_queries(&self.root.get_untracked(), &self.db_queries.get_untracked());
    }

    fn db_persist(&self) {
        let root = self.root.get_untracked();
        let configs: Vec<e_db::DbConfig> = self
            .db_conns
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
            .db_conns
            .with_untracked(|c| c.iter().any(|e| e.key() == key))
        {
            return;
        }
        let entry = DbEntry::new(self.cx, cfg);
        self.db_conns.update(|c| c.push(entry.clone()));
        self.db_persist();
        self.db_connect(entry);
    }

    pub fn db_remove(&self, key: String) {
        self.db_conns.update(|c| c.retain(|e| e.key() != key));
        self.db_persist();
    }

    pub fn db_connect(&self, entry: DbEntry) {
        if entry.connecting.get_untracked() {
            return;
        }
        entry.connecting.set(true);
        entry.error.set(None);
        let cfg = entry.config.clone();
        let send = create_ext_action(
            self.cx,
            move |res: Result<(Arc<e_db::Conn>, Vec<String>), String>| {
                entry.connecting.set(false);
                match res {
                    Ok((conn, tables)) => {
                        entry.conn.set(Some(conn));
                        entry.tables.set(tables);
                        entry.expanded.set(true);
                    }
                    Err(e) => entry.error.set(Some(e)),
                }
            },
        );
        std::thread::spawn(move || {
            let res = e_db::connect(&cfg).and_then(|conn| {
                let conn = Arc::new(conn);
                let tables = e_db::tables(&conn)?;
                Ok((conn, tables))
            });
            send(res);
        });
    }

    pub fn db_disconnect(&self, entry: DbEntry) {
        entry.conn.set(None);
        entry.tables.set(Vec::new());
        entry.expanded.set(false);
    }

    pub fn db_toggle(&self, entry: DbEntry) {
        if entry.conn.get_untracked().is_some() {
            entry.expanded.update(|e| *e = !*e);
        } else {
            self.db_connect(entry);
        }
    }

    pub fn db_refresh_tables(&self, entry: DbEntry) {
        let Some(conn) = entry.conn.get_untracked() else {
            return;
        };
        let send = create_ext_action(self.cx, move |tables: Vec<String>| {
            entry.tables.set(tables);
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
        self.db_result_key.set(Some(entry.key()));
        self.db_result_table.set(Some(table.clone()));
        self.db_result_title
            .set(format!("{} · {}", entry.config.display_name(), table));
        self.db_subview.set("data".into());
        self.db_sort.set(None);
        self.db_filter.set(None);
        self.db_page.set(0);
        self.db_columns.set(Vec::new());
        self.db_result_open.set(true);
        self.db_load_columns(entry.clone(), table.clone());
        self.db_reload_table();
    }

    /// (Re)run the browse query for the current table, sort and page.
    pub fn db_reload_table(&self) {
        let (Some(key), Some(table)) = (
            self.db_result_key.get_untracked(),
            self.db_result_table.get_untracked(),
        ) else {
            return;
        };
        let Some(entry) = self
            .db_conns
            .with_untracked(|c| c.iter().find(|e| e.key() == key).cloned())
        else {
            return;
        };
        let Some(conn) = entry.conn.get_untracked() else {
            return;
        };
        let engine = entry.config.engine.clone();
        let page = self.db_page.get_untracked();
        let sort = self.db_sort.get_untracked();
        let filter = self.db_filter.get_untracked();
        self.set_console_sql({
            let by = sort.as_ref().map(|(c, a)| (c.as_str(), *a));
            let f = filter.as_ref().map(|(c, v)| (c.as_str(), v.as_deref()));
            e_db::browse_sql(&engine, &table, f, by, DB_PAGE, page * DB_PAGE)
        });
        self.db_result_loading.set(true);
        self.db_result_error.set(None);
        let send = create_ext_action(self.cx, {
            let state = *self;
            move |res: Result<e_db::QueryResult, String>| state.db_apply_result(res)
        });
        std::thread::spawn(move || {
            let by = sort.as_ref().map(|(c, a)| (c.as_str(), *a));
            let f = filter.as_ref().map(|(c, v)| (c.as_str(), v.as_deref()));
            let sql = e_db::browse_sql(&engine, &table, f, by, DB_PAGE, page * DB_PAGE);
            send(e_db::query(&conn, &sql, DB_PAGE));
        });
    }

    /// Filter the current table to the value in the cell open in the edit
    /// overlay (WHERE col = value / IS NULL), then reload.
    pub fn db_filter_to_cell(&self) {
        let Some((row, col, column)) = self.db_edit.get_untracked() else {
            return;
        };
        let value = self.db_result.with_untracked(|r| {
            r.as_ref().and_then(|r| {
                r.rows
                    .get(row)
                    .and_then(|row| row.get(col))
                    .cloned()
                    .flatten()
            })
        });
        self.db_edit.set(None);
        self.db_filter.set(Some((column, value)));
        self.db_page.set(0);
        self.db_reload_table();
    }

    /// Clear the active data-view filter and reload.
    pub fn db_clear_filter(&self) {
        if self.db_filter.get_untracked().is_none() {
            return;
        }
        self.db_filter.set(None);
        self.db_page.set(0);
        self.db_reload_table();
    }

    /// Open the "insert row" dialog, one field per column of the current table.
    pub fn db_begin_insert(&self) {
        if self.db_result_table.get_untracked().is_none() {
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
        let fields: Vec<InsertField> = self.db_columns.with_untracked(|cols| {
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
        self.db_insert_fields.set(fields);
        self.db_insert_open.set(true);
    }

    pub fn db_cancel_insert(&self) {
        self.db_insert_open.set(false);
    }

    /// Insert the row built in the dialog. Columns left blank (and not marked
    /// NULL) are omitted so database defaults / auto-increment apply.
    pub fn db_commit_insert(&self) {
        let Some((entry, conn, table, engine)) = self.db_edit_target() else {
            return;
        };
        if entry.read_only.get_untracked() {
            Self::notify("Read-only: this connection is protected from writes.");
            self.db_insert_open.set(false);
            return;
        }
        let values: Vec<(String, Option<String>)> = self.db_insert_fields.with_untracked(|fs| {
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
        let state = *self;
        let send = create_ext_action(self.cx, move |res: Result<u64, String>| match res {
            Ok(_) => {
                state.db_insert_open.set(false);
                Self::notify("Row inserted");
                state.db_reload_table();
            }
            Err(e) => {
                state.db_insert_open.set(false);
                state.db_result_error.set(Some(e));
            }
        });
        std::thread::spawn(move || {
            send(e_db::insert_row(&conn, &engine, &table, &values));
        });
    }

    /// Toggle the sort on a column (asc → desc → off) and reload.
    pub fn db_sort_by(&self, col: String) {
        let next = match self.db_sort.get_untracked() {
            Some((c, true)) if c == col => Some((col, false)),
            Some((c, false)) if c == col => None,
            _ => Some((col, true)),
        };
        self.db_sort.set(next);
        self.db_page.set(0);
        self.db_reload_table();
    }

    /// Move to the next/previous page when browsing a table.
    pub fn db_page_by(&self, delta: i64) {
        let cur = self.db_page.get_untracked() as i64;
        let next = (cur + delta).max(0) as usize;
        if next == self.db_page.get_untracked() {
            return;
        }
        // Don't page past the end (a short page means we're at the last one).
        if delta > 0 {
            let len = self
                .db_result
                .with_untracked(|r| r.as_ref().map(|r| r.rows.len()).unwrap_or(0));
            if len < DB_PAGE {
                return;
            }
        }
        self.db_page.set(next);
        self.db_reload_table();
    }

    pub fn db_set_subview(&self, view: &str) {
        self.db_subview.set(view.to_string());
    }

    fn db_load_columns(&self, entry: DbEntry, table: String) {
        let Some(conn) = entry.conn.get_untracked() else {
            return;
        };
        let send = create_ext_action(self.cx, {
            let state = *self;
            move |(cols, idx): (Vec<e_db::ColumnInfo>, Vec<e_db::IndexInfo>)| {
                state.db_columns.set(cols);
                state.db_indexes.set(idx);
            }
        });
        std::thread::spawn(move || {
            let cols = e_db::columns(&conn, &table).unwrap_or_default();
            let idx = e_db::indexes(&conn, &table).unwrap_or_default();
            send((cols, idx));
        });
    }

    /// Test the current add-form connection without saving it.
    pub fn db_test_connection(&self) {
        let cfg = self.db_form.get_untracked().to_config();
        self.db_test_state.set("testing".into());
        let send = create_ext_action(self.cx, {
            let state = *self;
            move |res: Result<(), String>| {
                state.db_test_state.set(match res {
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
        self.db_form.set(DbForm {
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
        });
        self.db_editing_key.set(Some(entry.key()));
        self.db_test_state.set(String::new());
        self.db_adding.set(true);
    }

    /// Save the add/edit form: either add a new connection or replace one.
    pub fn db_submit_form(&self) {
        let cfg = self.db_form.get_untracked().to_config();
        if let Some(old_key) = self.db_editing_key.get_untracked() {
            self.db_conns.update(|c| c.retain(|e| e.key() != old_key));
            self.db_editing_key.set(None);
        }
        self.db_form.set(DbForm::default());
        self.db_adding.set(false);
        self.db_test_state.set(String::new());
        self.db_add_config(cfg);
    }

    /// Export the current result grid to a CSV file.
    pub fn db_export_csv(&self) {
        let Some(result) = self.db_result.get_untracked() else {
            return;
        };
        if result.columns.is_empty() {
            return;
        }
        let opts = floem::file::FileDialogOptions::new()
            .title("Export results as CSV")
            .default_name("results.csv");
        floem::action::save_as(opts, move |info| {
            let Some(path) = info.and_then(|i| i.path.into_iter().next()) else {
                return;
            };
            let mut out = String::new();
            out.push_str(&result.columns.join(","));
            out.push('\n');
            for row in &result.rows {
                let cells: Vec<String> = row
                    .iter()
                    .map(|c| csv_escape(c.as_deref().unwrap_or("")))
                    .collect();
                out.push_str(&cells.join(","));
                out.push('\n');
            }
            let _ = std::fs::write(&path, out);
        });
    }

    /// Open a blank query editor for a connection.
    pub fn db_new_query(&self, entry: DbEntry) {
        if entry.conn.get_untracked().is_none() {
            self.db_connect(entry.clone());
        }
        self.db_result_key.set(Some(entry.key()));
        self.db_result_title
            .set(format!("{} · query", entry.config.display_name()));
        self.db_result.set(None);
        self.db_result_error.set(None);
        if self.db_query_text.with_untracked(|q| q.trim().is_empty()) {
            self.set_console_sql("SELECT 1".into());
        }
        self.db_result_open.set(true);
    }

    /// Run the SQL currently in the query editor against the bound connection.
    /// Run the whole console (all statements).
    pub fn db_run_query(&self) {
        self.run_console_sql(self.db_query_text.get_untracked());
    }

    /// Run just the selected text, or — with no selection — the statement under
    /// the cursor. Falls back to the whole console if neither resolves.
    pub fn run_console_under_cursor(&self) {
        let text = self.db_query_text.get_untracked();
        let sql = self
            .db_console_editor
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
        self.run_console_sql(sql);
    }

    /// Gate a console run: destructive statements (any environment) or writes to
    /// a non-local database require explicit confirmation first (DB-702/703).
    fn run_console_sql(&self, sql: String) {
        let Some(entry) = self.db_result_key.get_untracked().and_then(|key| {
            self.db_conns
                .with_untracked(|c| c.iter().find(|e| e.key() == key).cloned())
        }) else {
            return;
        };
        let statements = e_db::split_statements(&sql);
        if statements.is_empty() {
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
            self.db_confirm.set(Some(crate::state::DbConfirm {
                verb: "Run".into(),
                statements: flagged,
                env,
                needs_ack: non_local,
                ack: self.cx.create_rw_signal(false),
                run: crate::state::ConfirmRun::Console(sql),
            }));
        }
    }

    /// Confirm and run the pending action (console SQL or a submit transaction).
    pub fn db_confirm_run(&self) {
        let Some(c) = self.db_confirm.get_untracked() else {
            return;
        };
        if c.needs_ack && !c.ack.get_untracked() {
            Self::notify("Tick the acknowledgement to proceed");
            return;
        }
        self.db_confirm.set(None);
        match c.run {
            crate::state::ConfirmRun::Console(sql) => self.execute_console_sql(sql),
            crate::state::ConfirmRun::Transaction(stmts) => self.execute_submit(stmts),
        }
    }

    /// Dismiss the confirmation dialog without running.
    pub fn db_confirm_cancel(&self) {
        self.db_confirm.set(None);
    }

    /// Run `sql` (which may contain multiple statements) against the active
    /// connection, one result tab per statement. Assumes confirmation (if any)
    /// has already been given.
    fn execute_console_sql(&self, sql: String) {
        let Some(key) = self.db_result_key.get_untracked() else {
            return;
        };
        let Some(entry) = self
            .db_conns
            .with_untracked(|c| c.iter().find(|e| e.key() == key).cloned())
        else {
            return;
        };
        let Some(conn) = entry.conn.get_untracked() else {
            self.db_result_error.set(Some("Not connected".into()));
            return;
        };
        // Split into individual statements so each gets its own result tab.
        let statements = e_db::split_statements(&sql);
        if statements.is_empty() {
            return;
        }
        self.db_result_loading.set(true);
        self.db_result_error.set(None);
        // Bump the run generation so any prior in-flight query (or a cancel) is
        // ignored when it returns.
        self.db_run_gen.update(|g| *g += 1);
        let gen = self.db_run_gen.get_untracked();
        // Metadata for the query-history log (written off the UI thread).
        let project = self.root.get_untracked().to_string_lossy().into_owned();
        let conn_label = entry.config.display_name();
        let history_path = crate::config::history_db_path();
        let key = entry.key();
        let send = create_ext_action(self.cx, {
            let state = *self;
            move |results: Vec<Result<e_db::QueryResult, String>>| {
                // Discard if cancelled or superseded by a newer run.
                if state.db_run_gen.get_untracked() != gen {
                    return;
                }
                state.db_build_console_tabs(key.clone(), results);
            }
        });
        std::thread::spawn(move || {
            let mut out = Vec::with_capacity(statements.len());
            for stmt in &statements {
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
                out.push(res);
            }
            send(out);
        });
    }

    /// Build console result tabs from a multi-statement run: keep pinned tabs,
    /// replace unpinned ones with the new results, and activate the first new tab.
    fn db_build_console_tabs(&self, key: String, results: Vec<Result<e_db::QueryResult, String>>) {
        self.db_result_loading.set(false);
        let mut tabs: Vec<crate::state::ResultTab> = self
            .db_result_tabs
            .get_untracked()
            .into_iter()
            .filter(|t| t.pinned)
            .collect();
        let first_new = tabs.len();
        for (i, res) in results.into_iter().enumerate() {
            let (result, error) = match res {
                Ok(r) => (Some(r), None),
                Err(e) => (None, Some(e)),
            };
            tabs.push(crate::state::ResultTab {
                title: format!("Result {}", i + 1),
                result,
                error,
                pinned: false,
                key: Some(key.clone()),
            });
        }
        self.db_result_tabs.set(tabs);
        // Console results aren't tied to a table, so they're read-only.
        self.db_result_table.set(None);
        self.db_columns.set(Vec::new());
        self.db_activate_tab(first_new);
    }

    /// Cancel the in-flight query: free the UI and discard the pending result
    /// when it eventually returns. (The query itself runs to completion in the
    /// background — server-side KILL is a future refinement.)
    pub fn db_cancel_query(&self) {
        self.db_run_gen.update(|g| *g += 1);
        self.db_result_loading.set(false);
        Self::notify("Query cancelled");
    }

    /// Show the result stored in tab `i` in the grid.
    pub fn db_activate_tab(&self, i: usize) {
        let tabs = self.db_result_tabs.get_untracked();
        let Some(tab) = tabs.get(i) else {
            return;
        };
        self.db_active_tab.set(i);
        self.db_result_key.set(tab.key.clone());
        self.db_result.set(tab.result.clone());
        self.db_result_error.set(tab.error.clone());
        self.db_selected_cell.set(None);
    }

    /// Toggle whether tab `i` is pinned (survives the next run).
    pub fn db_toggle_pin(&self, i: usize) {
        self.db_result_tabs.update(|tabs| {
            if let Some(t) = tabs.get_mut(i) {
                t.pinned = !t.pinned;
            }
        });
    }

    /// Close result tab `i`.
    pub fn db_close_tab(&self, i: usize) {
        self.db_result_tabs.update(|tabs| {
            if i < tabs.len() {
                tabs.remove(i);
            }
        });
        let len = self.db_result_tabs.with_untracked(|t| t.len());
        if len == 0 {
            self.db_result.set(None);
            self.db_result_error.set(None);
            self.db_active_tab.set(0);
        } else {
            let active = self.db_active_tab.get_untracked().min(len - 1);
            self.db_activate_tab(active);
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
        let Some(entry) = self.db_conns.with_untracked(|cs| {
            cs.iter()
                .find(|e| e.conn.get_untracked().is_some())
                .cloned()
        }) else {
            Self::notify("Run SQL: no connected database — connect one in the Database panel (⌘3)");
            return None;
        };
        Some((entry, sql))
    }

    pub fn run_sql_under_cursor(&self) {
        let Some((entry, sql)) = self.sql_and_conn_under_cursor() else {
            return;
        };
        self.db_result_key.set(Some(entry.key()));
        self.db_result_title.set(format!(
            "{} · query under cursor",
            entry.config.display_name()
        ));
        self.set_console_sql(sql);
        self.db_result.set(None);
        self.db_result_error.set(None);
        self.db_result_open.set(true);
        self.db_run_query();
    }

    /// Run EXPLAIN on the SQL under the cursor, show the plan in the result
    /// overlay, and flag full scans / missing indexes.
    pub fn explain_sql_under_cursor(&self) {
        let Some((entry, sql)) = self.sql_and_conn_under_cursor() else {
            return;
        };
        let Some(conn) = entry.conn.get_untracked() else {
            return;
        };
        let engine = entry.config.engine.clone();
        self.db_result_key.set(Some(entry.key()));
        self.db_result_title
            .set(format!("{} · EXPLAIN", entry.config.display_name()));
        self.db_result.set(None);
        self.db_result_error.set(None);
        self.db_result_loading.set(true);
        self.db_result_open.set(true);
        let state = *self;
        let send = create_ext_action(self.cx, move |res: Result<e_db::QueryResult, String>| {
            if let Ok(plan) = &res {
                let issues = e_db::analyze_explain(&engine, plan);
                if !issues.is_empty() {
                    Self::notify(&format!(
                        "EXPLAIN: {} — run “Suggest Index” to ask the agent for a migration",
                        issues.join("; ")
                    ));
                }
            }
            state.db_apply_result(res);
        });
        std::thread::spawn(move || send(e_db::explain(&conn, &sql)));
    }

    /// Run EXPLAIN on the SQL under the cursor and, if it has performance red
    /// flags, ask the agent to propose an index migration.
    pub fn suggest_index_under_cursor(&self) {
        let Some((entry, sql)) = self.sql_and_conn_under_cursor() else {
            return;
        };
        let Some(conn) = entry.conn.get_untracked() else {
            return;
        };
        let engine = entry.config.engine.clone();
        let sql_for_explain = sql.clone();
        let state = *self;
        let ask = create_ext_action(self.cx, move |issues: Vec<String>| {
            if issues.is_empty() {
                Self::notify("EXPLAIN found no full scans — no index needed");
                return;
            }
            state.send_to_agent(&format!(
                "This SQL query has a performance problem. Propose a Laravel migration that adds \
                 the missing index(es), then briefly explain why.\n\nQuery:\n{sql}\n\nEXPLAIN \
                 findings:\n- {}",
                issues.join("\n- ")
            ));
        });
        std::thread::spawn(move || {
            let issues = e_db::explain(&conn, &sql_for_explain)
                .map(|plan| e_db::analyze_explain(&engine, &plan))
                .unwrap_or_default();
            ask(issues);
        });
    }

    fn db_apply_result(&self, res: Result<e_db::QueryResult, String>) {
        self.db_result_loading.set(false);
        self.db_selected_cell.set(None);
        // Browsing a table is a single result; drop any console result tabs.
        self.db_result_tabs.set(Vec::new());
        // Pending edits are keyed by row/col of the *current* result, which just
        // changed — clear them so we never write against stale indices.
        self.db_pending_edits.update(|m| m.clear());
        self.db_pending_deletes.update(|m| m.clear());
        match res {
            Ok(r) => {
                self.db_result_error.set(None);
                self.db_result.set(Some(r));
            }
            Err(e) => {
                self.db_result.set(None);
                self.db_result_error.set(Some(e));
            }
        }
    }

    pub fn close_db_result(&self) {
        self.db_result_open.set(false);
        self.db_edit.set(None);
    }

    /// The user approved an agent-proposed query: run it and reply.
    pub fn db_consent_allow(&self) {
        let Some(c) = self.db_consent.get_untracked() else {
            return;
        };
        self.db_consent.set(None);
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
        if let Some(c) = self.db_consent.get_untracked() {
            self.db_consent.set(None);
            let _ = c
                .reply
                .send(serde_json::json!({"ok": false, "error": "denied by user"}));
        }
    }

    /// Whether the current results grid supports inline editing (a browsed table
    /// in data view, with a known primary key).
    pub fn db_editable(&self) -> bool {
        self.db_result_table.get_untracked().is_some()
            && self.db_subview.get_untracked() == "data"
            && self
                .db_columns
                .with_untracked(|c| c.iter().any(|c| c.key == "PRI"))
    }

    /// Begin editing the cell at `(row, col)`.
    pub fn db_begin_edit(&self, row: usize, col: usize) {
        if !self.db_editable() {
            return;
        }
        let Some(result) = self.db_result.get_untracked() else {
            return;
        };
        let Some(cell) = result.rows.get(row).and_then(|r| r.get(col)) else {
            return;
        };
        let column = result.columns.get(col).cloned().unwrap_or_default();
        self.db_edit_null.set(cell.is_none());
        self.db_edit_value.set(cell.clone().unwrap_or_default());
        self.db_edit.set(Some((row, col, column)));
    }

    pub fn db_cancel_edit(&self) {
        self.db_edit.set(None);
    }

    /// Toggle write protection for a connection (production defaults to on).
    pub fn db_toggle_read_only(&self, entry: DbEntry) {
        let now = !entry.read_only.get_untracked();
        entry.read_only.set(now);
        Self::notify(if now {
            "Database set to read-only"
        } else {
            "Database writes enabled"
        });
    }

    /// Write the edited cell back to the database.
    pub fn db_commit_edit(&self) {
        let Some((row, col, column)) = self.db_edit.get_untracked() else {
            return;
        };
        let (Some(key), Some(table)) = (
            self.db_result_key.get_untracked(),
            self.db_result_table.get_untracked(),
        ) else {
            return;
        };
        let Some(entry) = self
            .db_conns
            .with_untracked(|c| c.iter().find(|e| e.key() == key).cloned())
        else {
            return;
        };
        let Some(_conn) = entry.conn.get_untracked() else {
            return;
        };
        // Write guard: refuse edits to a read-only (e.g. production) connection.
        if entry.read_only.get_untracked() {
            Self::notify(
                "Read-only: this connection is protected from writes (looks like production). \
                 Toggle read-only off in the Database panel to edit.",
            );
            self.db_edit.set(None);
            return;
        }
        let Some(result) = self.db_result.get_untracked() else {
            return;
        };
        let _ = table;
        // Primary key of this row (from its current values).
        let Some(pk) = self.db_row_pk_full(row, &result) else {
            Self::notify("Edit: table has no primary key — cannot edit safely");
            self.db_edit.set(None);
            return;
        };
        let is_null = self.db_edit_null.get_untracked();
        let value = self.db_edit_value.get_untracked();
        let new = if is_null { None } else { Some(value.clone()) };

        // Stage the edit (transactional): reflect it in the grid and record it as
        // pending; the write happens on Submit.
        self.db_result.update(|r| {
            if let Some(r) = r {
                if let Some(cell) = r.rows.get_mut(row).and_then(|row| row.get_mut(col)) {
                    *cell = new.clone();
                }
            }
        });
        self.db_pending_edits.update(|m| {
            m.insert(
                (row, col),
                crate::state::PendingEdit {
                    column: column.clone(),
                    pk,
                    new,
                },
            );
        });
        self.db_edit.set(None);
    }

    /// Primary-key `(name, value)` pairs for `row`, using the result's columns.
    fn db_row_pk_full(
        &self,
        row: usize,
        result: &e_db::QueryResult,
    ) -> Option<Vec<(String, Option<String>)>> {
        let pk_names: Vec<String> = self.db_columns.with_untracked(|cols| {
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
            self.db_result_key.get_untracked(),
            self.db_result_table.get_untracked(),
        ) else {
            return None;
        };
        let entry = self
            .db_conns
            .with_untracked(|c| c.iter().find(|e| e.key() == key).cloned())?;
        let conn = entry.conn.get_untracked()?;
        let engine = entry.config.engine.clone();
        Some((entry, conn, table, engine))
    }

    /// Primary-key `(name, value)` pairs for `row` in the current result grid.
    fn db_row_pk(&self, row: usize) -> Vec<(String, Option<String>)> {
        let Some(result) = self.db_result.get_untracked() else {
            return Vec::new();
        };
        let pk_names: Vec<String> = self.db_columns.with_untracked(|cols| {
            cols.iter()
                .filter(|c| c.key == "PRI")
                .map(|c| c.name.clone())
                .collect()
        });
        let mut pk = Vec::new();
        for name in &pk_names {
            if let Some(idx) = result.columns.iter().position(|c| c == name) {
                pk.push((name.clone(), result.rows[row].get(idx).cloned().flatten()));
            }
        }
        pk
    }

    /// Delete the row currently open in the edit overlay.
    pub fn db_delete_row(&self) {
        let Some((row, _, _)) = self.db_edit.get_untracked() else {
            return;
        };
        let Some((entry, ..)) = self.db_edit_target() else {
            return;
        };
        if entry.read_only.get_untracked() {
            Self::notify(
                "Read-only: this connection is protected from writes (looks like production). \
                 Toggle read-only off in the Database panel to delete.",
            );
            self.db_edit.set(None);
            return;
        }
        let pk = self.db_row_pk(row);
        if pk.is_empty() {
            Self::notify("Delete: table has no primary key — cannot delete safely");
            return;
        }
        // Stage the deletion (transactional): mark the row pending; the DELETE
        // runs on Submit. Toggling again un-marks it.
        self.db_pending_deletes.update(|m| {
            if m.remove(&row).is_none() {
                m.insert(row, pk);
            }
        });
        self.db_edit.set(None);
    }

    /// Discard all staged changes and reload the table from the database.
    pub fn db_revert_changes(&self) {
        self.db_pending_edits.update(|m| m.clear());
        self.db_pending_deletes.update(|m| m.clear());
        self.db_reload_table();
    }

    /// Review staged changes and (after confirmation) run them in one
    /// transaction. Builds UPDATE/DELETE statements from the pending sets.
    pub fn db_submit_changes(&self) {
        let (Some(key), Some(table)) = (
            self.db_result_key.get_untracked(),
            self.db_result_table.get_untracked(),
        ) else {
            return;
        };
        let Some(entry) = self
            .db_conns
            .with_untracked(|c| c.iter().find(|e| e.key() == key).cloned())
        else {
            return;
        };
        let engine = entry.config.engine.clone();
        let mut stmts: Vec<String> = Vec::new();
        self.db_pending_edits.with_untracked(|edits| {
            for e in edits.values() {
                stmts.push(e_db::update_sql(
                    &engine,
                    &table,
                    &e.column,
                    e.new.as_deref(),
                    &e.pk,
                ));
            }
        });
        self.db_pending_deletes.with_untracked(|dels| {
            for pk in dels.values() {
                stmts.push(e_db::delete_sql(&engine, &table, pk));
            }
        });
        if stmts.is_empty() {
            return;
        }
        let env = entry.config.environment();
        self.db_confirm.set(Some(crate::state::DbConfirm {
            verb: "Submit".into(),
            statements: stmts.clone(),
            env,
            needs_ack: !env.is_local(),
            ack: self.cx.create_rw_signal(false),
            run: crate::state::ConfirmRun::Transaction(stmts),
        }));
    }

    /// Run the staged statements as one transaction, then refresh.
    fn execute_submit(&self, stmts: Vec<String>) {
        let Some(key) = self.db_result_key.get_untracked() else {
            return;
        };
        let Some(entry) = self
            .db_conns
            .with_untracked(|c| c.iter().find(|e| e.key() == key).cloned())
        else {
            return;
        };
        let Some(conn) = entry.conn.get_untracked() else {
            return;
        };
        self.db_result_loading.set(true);
        let state = *self;
        let send = create_ext_action(self.cx, move |res: Result<u64, String>| {
            state.db_result_loading.set(false);
            match res {
                Ok(n) => {
                    state.db_pending_edits.update(|m| m.clear());
                    state.db_pending_deletes.update(|m| m.clear());
                    Self::notify(&format!("Submitted — {n} row(s) affected"));
                    state.db_reload_table();
                }
                Err(e) => state.db_result_error.set(Some(e)),
            }
        });
        std::thread::spawn(move || {
            send(e_db::execute_transaction(&conn, &stmts));
        });
    }

    /// Hop to the foreign-key target of the column open in the edit overlay,
    /// filtered to the current cell's value.
    pub fn db_hop_fk(&self) {
        let Some((row, col, column)) = self.db_edit.get_untracked() else {
            return;
        };
        let Some((entry, conn, table, engine)) = self.db_edit_target() else {
            return;
        };
        let value = self.db_result.with_untracked(|r| {
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
        // `entry` holds non-Send signals, so it stays on the UI thread (in this
        // closure); only the ref-table name + query result cross the channel.
        let send = create_ext_action(
            self.cx,
            move |out: Option<(String, Result<e_db::QueryResult, String>)>| match out {
                None => Self::notify("No foreign key on this column"),
                Some((ref_table, res)) => {
                    state.db_edit.set(None);
                    state.db_result_key.set(Some(key.clone()));
                    state.db_result_table.set(Some(ref_table.clone()));
                    state.db_result_title.set(format!("{disp} · {ref_table}"));
                    state.db_subview.set("data".into());
                    state.db_sort.set(None);
                    state.db_filter.set(None);
                    state.db_page.set(0);
                    state.db_columns.set(Vec::new());
                    state.db_load_columns(entry.clone(), ref_table);
                    state.db_apply_result(res);
                }
            },
        );
        std::thread::spawn(move || {
            let out = match e_db::fk_target(&conn, &table, &column) {
                Ok(Some((ref_table, ref_col))) => {
                    let res = e_db::rows_where(
                        &conn,
                        &engine,
                        &ref_table,
                        &ref_col,
                        value.as_deref(),
                        DB_PAGE,
                    );
                    Some((ref_table, res))
                }
                _ => None,
            };
            send(out);
        });
    }
}
