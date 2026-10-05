//! Generating the Eloquent helper from the editor (see
//! [`e_laravel::eloquent_helper`] for what is generated and why).

use std::collections::HashMap;

use e_db::ColumnInfo;
use floem::reactive::SignalGet;

pub use e_laravel::eloquent_helper::*;

use crate::state::AppState;

impl AppState {
    /// **Laravel: Generate Eloquent Helper** — write the helper and say so.
    pub fn generate_eloquent_helper(&self) {
        self.regenerate_eloquent_helper(true);
    }

    /// After a schema reload: keep the helper current, if the project has one.
    /// Its presence is the opt-in; nothing is written to a project that never
    /// asked for it.
    pub fn maybe_regenerate_eloquent_helper(&self) {
        if self.root.get_untracked().join(FILE_NAME).is_file() {
            self.regenerate_eloquent_helper(false);
        }
    }

    fn regenerate_eloquent_helper(&self, announce: bool) {
        let root = self.root.get_untracked();
        if !crate::laravel::is_laravel(&root) {
            if announce {
                Self::notify("Not a Laravel project");
            }
            return;
        }
        let cached = self.db.schema_cache.get_untracked();
        let app = *self;
        self.spawn_bg(
            move || -> Result<(std::path::PathBuf, usize), String> {
                // Relations are cross-checked against the foreign keys, and the
                // schema comes from the connection when the cache is still cold.
                let conn = e_db::from_env(&root).and_then(|c| e_db::connect(&c).ok());
                let fks = conn
                    .as_ref()
                    .and_then(|c| e_db::foreign_keys(c).ok())
                    .unwrap_or_default();
                let nodes = crate::relations::build_graph(&root, &fks);
                let fresh: HashMap<String, Vec<ColumnInfo>>;
                let schema: &HashMap<String, Vec<ColumnInfo>> = if cached.is_empty() {
                    let mut map = HashMap::new();
                    if let Some(c) = &conn {
                        for t in e_db::tables(c).unwrap_or_default() {
                            if let Ok(cols) = e_db::columns(c, &t) {
                                map.insert(t, cols);
                            }
                        }
                    }
                    fresh = map;
                    &fresh
                } else {
                    &cached
                };
                let models = build(&root, &nodes, schema);
                if models.is_empty() {
                    return Err("no Eloquent models found under app/Models".into());
                }
                let path = root.join(FILE_NAME);
                std::fs::write(&path, render(&models)).map_err(|e| e.to_string())?;
                Ok((path, models.len()))
            },
            move |res: Result<(std::path::PathBuf, usize), String>| match res {
                Ok((path, n)) => {
                    // The servers index it now, not at their next start.
                    let uri = e_lsp::path_to_uri(&path);
                    for client in app.lsp_clients_for(e_core::language::Language::Php) {
                        client.did_change_watched_files(&[(uri.clone(), 2)]);
                    }
                    eprintln!("e: wrote {FILE_NAME} for {n} models");
                    if announce {
                        Self::notify(&format!(
                            "Eloquent helper written for {n} models ({FILE_NAME}) — add it to .gitignore"
                        ));
                    }
                }
                Err(e) => {
                    eprintln!("e: Eloquent helper: {e}");
                    if announce {
                        Self::notify(&format!("Eloquent helper: {e}"));
                    }
                }
            },
        );
    }
}
