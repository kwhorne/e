//! Database panel state: connections, table browsing, querying and inline cell
//! editing across MySQL, PostgreSQL, SQLite and ClickHouse.
//!
//! The view lives in [`crate::db_view`]; this module owns the `AppState` methods
//! that drive it. Extracted from the former `state.rs` god-module (fields stay on
//! `AppState`); same pattern as [`crate::debug`] / [`crate::runtime`].

use std::sync::Arc;

use floem::ext_event::create_ext_action;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};

use crate::state::{AppState, DbEntry, DbForm};

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
        self.db_query_text.set(sql);
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
        self.db_query_text.set({
            let by = sort.as_ref().map(|(c, a)| (c.as_str(), *a));
            e_db::browse_sql(&engine, &table, by, DB_PAGE, page * DB_PAGE)
        });
        self.db_result_loading.set(true);
        self.db_result_error.set(None);
        let send = create_ext_action(self.cx, {
            let state = *self;
            move |res: Result<e_db::QueryResult, String>| state.db_apply_result(res)
        });
        std::thread::spawn(move || {
            let by = sort.as_ref().map(|(c, a)| (c.as_str(), *a));
            let sql = e_db::browse_sql(&engine, &table, by, DB_PAGE, page * DB_PAGE);
            send(e_db::query(&conn, &sql, DB_PAGE));
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
            move |cols: Vec<e_db::ColumnInfo>| state.db_columns.set(cols)
        });
        std::thread::spawn(move || {
            send(e_db::columns(&conn, &table).unwrap_or_default());
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
            self.db_query_text.set("SELECT 1".into());
        }
        self.db_result_open.set(true);
    }

    /// Run the SQL currently in the query editor against the bound connection.
    pub fn db_run_query(&self) {
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
        let sql = self.db_query_text.get_untracked();
        if sql.trim().is_empty() {
            return;
        }
        self.db_result_loading.set(true);
        self.db_result_error.set(None);
        let send = create_ext_action(self.cx, {
            let state = *self;
            move |res: Result<e_db::QueryResult, String>| state.db_apply_result(res)
        });
        std::thread::spawn(move || {
            send(e_db::query(&conn, &sql, e_db::MAX_ROWS));
        });
    }

    fn db_apply_result(&self, res: Result<e_db::QueryResult, String>) {
        self.db_result_loading.set(false);
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
        let Some(conn) = entry.conn.get_untracked() else {
            return;
        };
        let Some(result) = self.db_result.get_untracked() else {
            return;
        };
        // Build the primary-key conditions from the row's current values.
        let pk_names: Vec<String> = self.db_columns.with_untracked(|cols| {
            cols.iter()
                .filter(|c| c.key == "PRI")
                .map(|c| c.name.clone())
                .collect()
        });
        let mut pk: Vec<(String, Option<String>)> = Vec::new();
        for name in &pk_names {
            if let Some(idx) = result.columns.iter().position(|c| c == name) {
                pk.push((name.clone(), result.rows[row].get(idx).cloned().flatten()));
            }
        }
        let engine = entry.config.engine.clone();
        let is_null = self.db_edit_null.get_untracked();
        let value = self.db_edit_value.get_untracked();
        let set_val = if is_null { None } else { Some(value.clone()) };

        let state = *self;
        let send = create_ext_action(self.cx, move |res: Result<u64, String>| match res {
            Ok(_) => {
                // Reflect the change in the in-memory grid.
                state.db_result.update(|r| {
                    if let Some(r) = r {
                        if let Some(cell) = r.rows.get_mut(row).and_then(|row| row.get_mut(col)) {
                            *cell = if is_null { None } else { Some(value.clone()) };
                        }
                    }
                });
                state.db_edit.set(None);
            }
            Err(e) => state.db_result_error.set(Some(e)),
        });
        std::thread::spawn(move || {
            let pk_ref = pk;
            send(e_db::update_cell(
                &conn,
                &engine,
                &table,
                &column,
                set_val.as_deref(),
                &pk_ref,
            ));
        });
    }
}
