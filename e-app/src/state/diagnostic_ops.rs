//! Diagnostics: publishing, waiting for and counting them, lint and PHPStan.

use super::*;

impl AppState {
    /// Record one server's diagnostics for a file and rebuild the merged view.
    /// Keyed per server, so intelephense and laravel-lsp can both report on the
    /// same file without overwriting each other.
    pub fn publish_diagnostics(&self, server_id: &str, uri: &str, diags: Vec<Diagnostic>) {
        let key = (server_id.to_string(), uri.to_string());
        self.problems.by_server.update(|m| {
            if diags.is_empty() {
                m.remove(&key);
            } else {
                m.insert(key, diags);
            }
        });
        let merged: Vec<Diagnostic> = self.problems.by_server.with_untracked(|m| {
            m.iter()
                .filter(|((_, u), _)| u == uri)
                .flat_map(|(_, d)| d.iter().cloned())
                .collect()
        });
        self.problems.by_file.update(|map| {
            if merged.is_empty() {
                map.remove(uri);
            } else {
                map.insert(uri.to_string(), merged.clone());
            }
        });
        self.apply_diagnostics_to_buffer(uri, &merged);
        self.resolve_diag_waiters(server_id, uri);
    }

    /// The diagnostics for `uri` in the shape the agent socket speaks.
    pub(crate) fn diagnostics_json_for(&self, uri: &str) -> Vec<serde_json::Value> {
        let path = uri_to_path(uri).to_string_lossy().into_owned();
        let mut diags = self
            .problems
            .by_file
            .with_untracked(|map| map.get(uri).cloned().unwrap_or_default());
        diags.extend(self.own_diagnostics(uri));
        diags
            .iter()
            .map(|d| {
                let severity = match d.severity {
                    Some(lsp_types::DiagnosticSeverity::ERROR) => "error",
                    Some(lsp_types::DiagnosticSeverity::WARNING) => "warning",
                    Some(lsp_types::DiagnosticSeverity::INFORMATION) => "info",
                    Some(lsp_types::DiagnosticSeverity::HINT) => "hint",
                    _ => "info",
                };
                serde_json::json!({
                    "file": path,
                    "line": d.range.start.line + 1,
                    "col": d.range.start.character + 1,
                    "severity": severity,
                    "source": d.source,
                    "message": d.message,
                })
            })
            .collect()
    }

    /// What `e` itself found in an open file, beside the language servers:
    /// its lints (unknown columns, missing views and keys), PHPStan, a
    /// pending migration's risks against the data, and exceptions the running
    /// app logged.
    pub(crate) fn own_diagnostics(&self, uri: &str) -> Vec<Diagnostic> {
        self.editor.buffers.with_untracked(|bs| {
            bs.iter()
                .find(|b| b.uri.as_deref() == Some(uri))
                .map(|b| {
                    let mut all = b.lint.borrow().clone();
                    all.extend(b.analysis.borrow().iter().cloned());
                    all.extend(b.migration_risks.borrow().iter().cloned());
                    all.extend(b.caught.borrow().iter().cloned());
                    all
                })
                .unwrap_or_default()
        })
    }

    /// An agent wrote `path` and wants to know what the language servers make
    /// of it: reload the buffer (open it if needed), then answer once every
    /// running server for the file has re-published — or at `timeout`, with
    /// whatever is known then. The edit → diagnostics → fix loop, without the
    /// agent guessing how long to sleep.
    pub fn agent_wait_diagnostics(
        &self,
        path: PathBuf,
        timeout: std::time::Duration,
        reply: std::sync::mpsc::Sender<serde_json::Value>,
    ) {
        let root = self.root.get_untracked();
        let abs = if path.is_absolute() {
            path
        } else {
            root.join(path)
        };
        let abs = abs.canonicalize().unwrap_or(abs);
        let uri = path_to_uri(&abs);
        let language = e_core::language::Language::from_path(&abs);

        let open = self.editor.buffers.with_untracked(|bs| {
            bs.iter()
                .find(|b| b.file.path.as_deref() == Some(abs.as_path()))
                .cloned()
        });
        let answer_now = |note: &str| {
            let _ = reply.send(serde_json::json!({
                "ok": true,
                "path": abs.to_string_lossy(),
                "diagnostics": self.diagnostics_json_for(&uri),
                "complete": true,
                "note": note,
            }));
        };
        match &open {
            Some(b) if b.dirty.get_untracked() => {
                answer_now("buffer has unsaved changes; not reloaded from disk");
                return;
            }
            Some(b) => {
                let on_disk = std::fs::read_to_string(&abs).unwrap_or_default();
                if on_disk == b.doc.text().to_string() {
                    // Nothing changed, so nothing new is coming: answer with
                    // what the servers already said.
                    answer_now("file unchanged");
                    return;
                }
                self.reload_buffer(b);
            }
            None => self.open_path(abs.clone()),
        }

        let pending: HashSet<String> =
            lsp_registry::server_specs(language, self.is_laravel_project())
                .iter()
                .filter(|s| {
                    self.lsp
                        .clients
                        .with_untracked(|m| m.get(s.id).map(|c| c.is_alive()).unwrap_or(false))
                })
                .map(|s| s.id.to_string())
                .collect();
        if pending.is_empty() {
            answer_now("no language server for this file");
            return;
        }
        let id = self.diag_waiter_seq.get_untracked() + 1;
        self.diag_waiter_seq.set(id);
        self.problems.waiters.update(|w| {
            w.push(DiagWaiter {
                id,
                uri: uri.clone(),
                pending,
                reply,
            })
        });
        let app = *self;
        floem::action::exec_after(timeout, move |_| {
            let waiter = app.problems.waiters.try_update(|w| {
                let pos = w.iter().position(|x| x.id == id)?;
                Some(w.remove(pos))
            });
            if let Some(Some(w)) = waiter {
                let _ = w.reply.send(serde_json::json!({
                    "ok": true,
                    "path": uri_to_path(&w.uri).to_string_lossy(),
                    "diagnostics": app.diagnostics_json_for(&w.uri),
                    "complete": false,
                    "note": format!("timed out waiting for {}", w.pending.iter().cloned().collect::<Vec<_>>().join(", ")),
                }));
            }
        });
    }

    /// `server_id` published for `uri`: waiters on that file hear from it, and
    /// are answered once every server they were waiting for has spoken.
    pub(crate) fn resolve_diag_waiters(&self, server_id: &str, uri: &str) {
        if self.problems.waiters.with_untracked(|w| w.is_empty()) {
            return;
        }
        let done: Vec<DiagWaiter> = self
            .problems
            .waiters
            .try_update(|w| {
                let mut done = Vec::new();
                w.retain_mut(|x| {
                    if x.uri != uri {
                        return true;
                    }
                    x.pending.remove(server_id);
                    if x.pending.is_empty() {
                        done.push(x.clone());
                        false
                    } else {
                        true
                    }
                });
                done
            })
            .unwrap_or_default();
        for w in done {
            let _ = w.reply.send(serde_json::json!({
                "ok": true,
                "path": uri_to_path(&w.uri).to_string_lossy(),
                "diagnostics": self.diagnostics_json_for(&w.uri),
                "complete": true,
            }));
        }
    }

    /// Rebuild a buffer's inline diagnostic spans and repaint it.
    pub fn apply_diagnostics_to_buffer(&self, uri: &str, diags: &[Diagnostic]) {
        let Some(buf) = self
            .editor
            .buffers
            .with(|bs| bs.iter().find(|b| b.uri.as_deref() == Some(uri)).cloned())
        else {
            return;
        };
        let text = buf.doc.text().to_string();
        // Merge the LSP diagnostics with our Laravel query lint.
        let mut all = diags.to_vec();
        all.extend(buf.lint.borrow().iter().cloned());
        all.extend(buf.analysis.borrow().iter().cloned());
        all.extend(buf.migration_risks.borrow().iter().cloned());
        all.extend(buf.caught.borrow().iter().cloned());
        *buf.diag_lines.borrow_mut() = build_diag_lines(&all, &text);
        buf.doc.cache_rev().update(|r| *r += 1);
    }

    /// Run PHPStan over one file and attach its findings to the buffer.
    ///
    /// Off the UI thread, and a no-op unless the project actually ships PHPStan
    /// with a config — analysing a project that never opted in would be slow and
    /// wrong. Analysing the single saved file rather than the whole project
    /// keeps it to something worth doing on every save.
    pub fn run_phpstan(&self, buffer_id: u64) {
        // `vendor/bin/phpstan` is the project's code.
        if !self.trusted.get_untracked() {
            return;
        }
        let Some(buf) = self.buffer_by_id(buffer_id) else {
            return;
        };
        if !matches!(buf.file.language, Language::Php) {
            return;
        }
        let (Some(path), Some(uri)) = (buf.file.path.clone(), buf.uri.clone()) else {
            return;
        };
        let root = self.root.get_untracked();
        let (Some(bin), true) = (
            crate::phptools::phpstan_binary(&root),
            crate::phptools::has_phpstan_config(&root),
        ) else {
            return;
        };

        let app = *self;
        let send =
            create_ext_action(self.cx, move |diags: Vec<Diagnostic>| {
                let Some(buf) = app.editor.buffers.with_untracked(|bs| {
                    bs.iter().find(|b| b.uri.as_deref() == Some(&uri)).cloned()
                }) else {
                    return;
                };
                if *buf.analysis.borrow() == diags {
                    return;
                }
                *buf.analysis.borrow_mut() = diags;
                let lsp = app
                    .problems
                    .by_file
                    .with_untracked(|m| m.get(&uri).cloned().unwrap_or_default());
                app.apply_diagnostics_to_buffer(&uri, &lsp);
            });

        std::thread::spawn(move || {
            let out = std::process::Command::new(&bin)
                .args(["analyse", "--error-format=json", "--no-progress"])
                .arg(&path)
                .current_dir(&root)
                .output();
            let Ok(out) = out else { return };
            let text = String::from_utf8_lossy(&out.stdout);
            let Some(report) = crate::phptools::parse_phpstan(&text) else {
                return;
            };
            for e in &report.errors {
                eprintln!("e: phpstan: {e}");
            }
            send(crate::phptools::diagnostics_for(&report, &path));
        });
    }

    /// Recompute `e`'s own lints for a buffer and re-render its diagnostics:
    /// unknown query-builder columns (live schema), missing views / routes /
    /// translation keys (project data — off when laravel-lsp runs, which does
    /// the same), `env()` outside `config/`, and `.env` keys `.env.example`
    /// declares. Debounced and off the
    /// UI thread: resolving a model's table reads its class file, and a
    /// keystroke shouldn't wait for disk.
    pub fn refresh_lint(&self, buffer_id: u64) {
        let Some(buf) = self.buffer_by_id(buffer_id) else {
            return;
        };
        let php_like = matches!(buf.file.language, Language::Php | Language::Blade);
        let is_env = buf
            .file
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n == ".env")
            .unwrap_or(false);
        // The project's own rules, on whatever files they cover, and the
        // rules file itself.
        let rules_rel = buf.file.path.as_ref().and_then(|p| self.rules_rel(p));
        let rules_file = buf
            .file
            .path
            .as_ref()
            .is_some_and(|p| *p == self.root.get_untracked().join(e_review::rules::FILE));
        if buf.large || !(php_like || is_env || rules_rel.is_some() || rules_file) {
            return;
        }
        let project_rules = self.project.rules.get_untracked();
        let schema = self.db.schema_cache.get_untracked();
        let laravel = if self.laravel_lsp_running() {
            None
        } else {
            self.project.laravel.get_untracked()
        };
        // Controllers get the unchecked-lookup check whether or not
        // laravel-lsp runs: it doesn't make it.
        let root_now = self.root.get_untracked();
        let controller_routes = buf
            .file
            .path
            .as_ref()
            .filter(|p| p.starts_with(root_now.join("app/Http/Controllers")))
            .and_then(|_| self.project.laravel.get_untracked());
        // `env()` outside `config/`: in a Laravel project, whatever else runs.
        let env_rel = buf
            .file
            .path
            .as_ref()
            .filter(|_| php_like && root_now.join("artisan").is_file())
            .and_then(|p| p.strip_prefix(&root_now).ok())
            .map(Path::to_path_buf);
        if php_like
            && schema.is_empty()
            && laravel.is_none()
            && controller_routes.is_none()
            && env_rel.is_none()
            && rules_rel.is_none()
        {
            return;
        }
        let gen = buf.lint_gen.get() + 1;
        buf.lint_gen.set(gen);
        let app = *self;
        floem::action::exec_after(LINT_DEBOUNCE, move |_| {
            let Some(buf) = app.buffer_by_id(buffer_id) else {
                return;
            };
            if buf.lint_gen.get() != gen {
                return; // typed again while waiting; that keystroke's run will do it
            }
            let text = buf.doc.text().to_string();
            let language = buf.file.language;
            let root = app.root.get_untracked();
            let lint_gen = buf.lint_gen.clone();
            app.spawn_bg(
                move || {
                    let mut diags = Vec::new();
                    if php_like {
                        if !schema.is_empty() {
                            diags.extend(column_lint(&text, &root, &schema));
                        }
                        if let Some(data) = &laravel {
                            diags.extend(crate::laravel_lint::lint(&text, language, data));
                        }
                        if let Some(data) = &controller_routes {
                            diags.extend(unchecked_lookup_lint(&text, &root, &data.all_routes));
                        }
                        if let Some(rel) = &env_rel {
                            diags.extend(e_laravel::env_guard::lint(rel, &text));
                        }
                    }
                    if let Some(rel) = &rules_rel {
                        diags.extend(crate::project_rules::lint(&project_rules, rel, &text));
                    }
                    if rules_file {
                        diags.extend(crate::project_rules::lint_rules_file(&text));
                    }
                    if is_env {
                        if let Ok(example) = std::fs::read_to_string(root.join(".env.example")) {
                            diags.extend(crate::laravel_lint::lint_env(&text, &example));
                        }
                    }
                    diags
                },
                move |diags: Vec<Diagnostic>| {
                    if lint_gen.get() != gen {
                        return; // the text moved on while we were linting
                    }
                    let Some(buf) = app.buffer_by_id(buffer_id) else {
                        return;
                    };
                    if *buf.lint.borrow() == diags {
                        return;
                    }
                    *buf.lint.borrow_mut() = diags;
                    if let Some(uri) = buf.uri.clone() {
                        let lsp = app
                            .problems
                            .by_file
                            .with_untracked(|m| m.get(&uri).cloned().unwrap_or_default());
                        app.apply_diagnostics_to_buffer(&uri, &lsp);
                    }
                },
            );
        });
    }

    /// `(line, col, selection_len)` of the active editor's cursor (1-based).
    /// Reactive: reads the cursor signal, so call it inside a view closure.
    pub fn cursor_info(&self) -> Option<(usize, usize, usize)> {
        let buf = self.active_buffer()?;
        let editor = buf.editor.get()?;
        let cursor = editor.cursor.get();
        let offset = cursor.offset();
        let (line, col) = editor.offset_to_line_col(offset);
        let sel_len = match &cursor.mode {
            CursorMode::Insert(sel) => sel.regions().iter().map(|r| r.max() - r.min()).sum(),
            _ => 0,
        };
        Some((line + 1, col + 1, sel_len))
    }

    /// `(errors, warnings)` for the active buffer.
    pub fn active_diagnostic_counts(&self) -> (usize, usize) {
        let Some(buf) = self.active_buffer() else {
            return (0, 0);
        };
        let Some(uri) = buf.uri.as_ref() else {
            return (0, 0);
        };
        self.problems.by_file.with(|map| {
            let Some(diags) = map.get(uri) else {
                return (0, 0);
            };
            let mut errors = 0;
            let mut warnings = 0;
            for d in diags {
                match d.severity {
                    Some(lsp_types::DiagnosticSeverity::ERROR) => errors += 1,
                    Some(lsp_types::DiagnosticSeverity::WARNING) => warnings += 1,
                    _ => {}
                }
            }
            (errors, warnings)
        })
    }

    /// All non-empty diagnostics across open files, grouped and sorted.
    pub fn all_diagnostics(&self) -> Vec<(String, Vec<Diagnostic>)> {
        self.problems.by_file.with(|map| {
            let mut groups: Vec<(String, Vec<Diagnostic>)> = map
                .iter()
                .filter(|(_, d)| !d.is_empty())
                .map(|(uri, d)| {
                    let mut dd = d.clone();
                    dd.sort_by_key(|x| x.range.start.line);
                    (uri.clone(), dd)
                })
                .collect();
            groups.sort_by(|a, b| a.0.cmp(&b.0));
            groups
        })
    }

    /// Total number of diagnostics across all open files.
    pub fn total_diagnostic_count(&self) -> usize {
        self.problems
            .by_file
            .with(|m| m.values().map(|v| v.len()).sum())
    }
}
