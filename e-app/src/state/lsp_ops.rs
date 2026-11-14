//! The language servers: starting and restarting them, workspace edits, formatting and code actions.

use super::*;

impl AppState {
    /// Look up a running language server for `language` (does not start one).
    pub fn lsp_for_language(&self, language: Language) -> Option<Arc<LspClient>> {
        let spec = lsp_registry::primary_spec(language, self.is_laravel_project())?;
        self.lsp
            .clients
            .with(|m| m.get(spec.id).cloned())
            .filter(|c| c.is_alive())
    }

    /// The language server for the active buffer, if running.
    pub fn lsp_for_active(&self) -> Option<Arc<LspClient>> {
        self.lsp_for_language(self.active_buffer()?.file.language)
    }

    /// Is the workspace a Laravel project? Cheap fs check, cached for the
    /// session (opening another project spawns a new window).
    pub(crate) fn is_laravel_project(&self) -> bool {
        if let Some(v) = self.laravel_project.get_untracked() {
            return v;
        }
        let st = self.settings.get_untracked();
        let v = st.laravel && st.laravel_lsp && laravel::is_laravel(&self.root.get_untracked());
        self.laravel_project.set(Some(v));
        v
    }

    /// Is the official Laravel language server up?
    pub(crate) fn laravel_lsp_running(&self) -> bool {
        self.lsp
            .clients
            .with(|m| m.get("laravel-lsp").map(|c| c.is_alive()))
            .unwrap_or(false)
    }

    /// The LSP `languageId` for `language`, or `None` when nothing handles it.
    pub(crate) fn lsp_language_id(&self, language: Language) -> Option<&'static str> {
        lsp_registry::language_id(language, self.is_laravel_project())
    }

    /// Every *running* server for `language` — requests that can merge (completion,
    /// code actions, document sync) go to all of them.
    pub fn lsp_clients_for(&self, language: Language) -> Vec<Arc<LspClient>> {
        let specs = lsp_registry::server_specs(language, self.is_laravel_project());
        self.lsp.clients.with(|m| {
            specs
                .iter()
                .filter_map(|s| m.get(s.id).cloned())
                .filter(|c| c.is_alive())
                .collect::<Vec<_>>()
        })
    }

    /// Every running server for the active buffer's language.
    pub fn lsp_all_for_active(&self) -> Vec<Arc<LspClient>> {
        match self.active_buffer() {
            Some(b) => self.lsp_clients_for(b.file.language),
            None => Vec::new(),
        }
    }

    /// Start (or reuse) the language server for `language`.
    pub(crate) fn ensure_lsp(&self, language: Language) -> Option<Arc<LspClient>> {
        let specs = lsp_registry::server_specs(language, self.is_laravel_project());
        // Start every server this language wants; a missing optional one (e.g.
        // laravel-lsp not installed) just means fewer features, never an error.
        for spec in &specs {
            self.ensure_one_lsp(spec);
        }
        specs
            .first()
            .and_then(|s| self.lsp.clients.with(|m| m.get(s.id).cloned()))
    }

    /// Start one server if it isn't already running, subject to the health
    /// policy: a missing binary is retried only once it shows up on `PATH`, and a
    /// server that gave up or failed to initialise stays down until the user asks.
    pub(crate) fn ensure_one_lsp(&self, spec: &lsp_registry::ServerSpec) -> Option<Arc<LspClient>> {
        if let Some(client) = self.lsp.clients.with(|m| m.get(spec.id).cloned()) {
            if client.is_alive() {
                return Some(client);
            }
            // A corpse whose exit event hasn't been handled yet; the restart
            // policy below decides what happens to it.
            self.lsp.clients.update(|m| {
                m.remove(spec.id);
            });
        }
        let restarts = match self.lsp.health.with(|h| h.get(spec.id).cloned()) {
            // Retried only when the binary has appeared, so installing it while
            // `e` runs is picked up on the next file open — and nobody is nagged
            // on every open until then.
            Some(ServerHealth::NotInstalled) if !program_on_path(spec.program) => return None,
            Some(ServerHealth::GaveUp { .. }) | Some(ServerHealth::Failed(_)) => return None,
            Some(ServerHealth::Running { restarts })
            | Some(ServerHealth::Starting { restarts }) => restarts,
            _ => 0,
        };
        self.start_lsp(spec, restarts)
    }

    /// Spawn one server and record how it went. `restarts` is carried into its
    /// health so a crash loop can be counted across restarts.
    pub(crate) fn start_lsp(
        &self,
        spec: &lsp_registry::ServerSpec,
        restarts: u32,
    ) -> Option<Arc<LspClient>> {
        // Servers that load the project's own code wait for it to be trusted;
        // trusting it starts them (`trust_project`).
        if !self.trusted.get_untracked() && !crate::trust::server_reads_only(spec.program) {
            return None;
        }
        let tx = self.lsp_tx.get();
        let queue = self.lsp.queue.get_untracked();
        // Tag events with the server that produced them so two servers on the
        // same file don't overwrite each other's diagnostics.
        let server_id = spec.id.to_string();
        let handler: e_lsp::EventHandler = Box::new(move |event| {
            if let Ok(mut q) = queue.lock() {
                q.push_back((server_id.clone(), event));
            }
            let _ = tx.send(());
        });
        let root = self.root.get();
        let settings = spec
            .settings
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
        match LspClient::start_with_settings(spec.program, spec.args, &root, settings, handler) {
            Ok(client) => {
                eprintln!("e: starting {} for {}", spec.id, root.display());
                self.lsp.clients.update(|m| {
                    m.insert(spec.id.to_string(), client.clone());
                });
                self.lsp.health.update(|h| {
                    h.insert(spec.id.to_string(), ServerHealth::Starting { restarts });
                });
                Some(client)
            }
            Err(e) => {
                // Not installed is the common case, and it has a one-line fix —
                // say what to run instead of reporting a bare spawn failure.
                let health = if is_not_found(&e) {
                    let msg = lsp_registry::missing_message(spec);
                    eprintln!("e: {msg}");
                    Self::notify(&format!(
                        "{msg} — or click it in the status bar, or run “Install {}” from ⌘⇧P",
                        spec.id
                    ));
                    ServerHealth::NotInstalled
                } else {
                    let msg = format!("could not start {} ({e:#})", spec.program);
                    eprintln!("e: {msg}");
                    Self::notify(&msg);
                    ServerHealth::Failed(format!("{e:#}"))
                };
                self.lsp.health.update(|h| {
                    h.insert(spec.id.to_string(), health);
                });
                None
            }
        }
    }

    /// Handle one event a language server pushed at us (on the UI thread).
    pub fn handle_lsp_event(&self, server_id: &str, event: ServerEvent) {
        match event {
            ServerEvent::Ready => {
                let restarts = match self
                    .lsp
                    .health
                    .with_untracked(|h| h.get(server_id).cloned())
                {
                    Some(ServerHealth::Starting { restarts }) => restarts,
                    Some(ServerHealth::Running { restarts }) => restarts,
                    _ => 0,
                };
                self.lsp.health.update(|h| {
                    h.insert(server_id.to_string(), ServerHealth::Running { restarts });
                });
                if let Some(c) = self
                    .lsp
                    .clients
                    .with_untracked(|m| m.get(server_id).cloned())
                {
                    eprintln!(
                        "e: {server_id} ready ({:?} columns, {} sync)",
                        c.position_encoding(),
                        if c.incremental_sync() {
                            "incremental"
                        } else {
                            "full"
                        }
                    );
                }
            }
            ServerEvent::InitFailed(reason) => {
                self.lsp.clients.update(|m| {
                    m.remove(server_id);
                });
                self.lsp.health.update(|h| {
                    h.insert(server_id.to_string(), ServerHealth::Failed(reason.clone()));
                });
                let msg = format!("{server_id} failed to initialise: {reason}");
                eprintln!("e: {msg}");
                Self::notify(&msg);
            }
            ServerEvent::Diagnostics(p) => {
                self.publish_diagnostics(server_id, p.uri.as_str(), p.diagnostics);
            }
            ServerEvent::ApplyEdit { label, edits } => {
                let n = self.apply_workspace_edits(&edits);
                let what = label.unwrap_or_else(|| "edit".to_string());
                eprintln!("e: {server_id}: applied {what} to {n} file(s)");
                if n > 0 {
                    Self::notify(&format!("{server_id}: {what}"));
                }
            }
            ServerEvent::Message { level, text } => match level {
                // A server saying "license invalid" or "project too large" is
                // exactly what the user needs to see; chatter stays on stderr.
                MessageLevel::Error | MessageLevel::Warning => {
                    eprintln!("e: {server_id}: {text}");
                    Self::notify(&format!("{server_id}: {text}"));
                }
                MessageLevel::Info | MessageLevel::Log => eprintln!("[lsp:{server_id}] {text}"),
            },
            ServerEvent::Progress {
                title,
                message,
                percentage,
                done,
            } => {
                if done {
                    // Intelephense answers anything asked during indexing with
                    // nothing, and caches that answer until a document event.
                    // Now that it knows the project, ask again for what the
                    // user is looking at.
                    self.request_outline();
                    self.request_inlay_hints_active();
                }
                self.lsp.progress.update(|p| {
                    if done {
                        p.remove(server_id);
                    } else {
                        let mut text = title;
                        if let Some(m) = message.filter(|m| !m.is_empty()) {
                            text = format!("{text} {m}");
                        }
                        if let Some(pct) = percentage {
                            text = format!("{text} {pct}%");
                        }
                        p.insert(server_id.to_string(), text.trim().to_string());
                    }
                });
            }
            ServerEvent::Exited => self.on_lsp_exited(server_id),
        }
    }

    /// A server process died on us. Bring it back — with every open document
    /// re-sent, since the new process knows nothing — unless it has done this
    /// too often, in which case leave it down and say so.
    pub(crate) fn on_lsp_exited(&self, id: &str) {
        // A client we shut down ourselves never reports this (see `closing` in
        // e-lsp), so an exit is always the live one dying — unless a newer
        // client has already taken the slot, in which case the event is stale.
        match self.lsp.clients.with_untracked(|m| m.get(id).cloned()) {
            Some(c) if c.is_alive() => return,
            Some(_) => {}
            None => return,
        }
        self.lsp.clients.update(|m| {
            m.remove(id);
        });
        self.lsp.progress.update(|p| {
            p.remove(id);
        });
        let restarts = match self.lsp.health.with_untracked(|h| h.get(id).cloned()) {
            Some(ServerHealth::Running { restarts })
            | Some(ServerHealth::Starting { restarts }) => restarts + 1,
            _ => 1,
        };
        if restarts > MAX_LSP_RESTARTS {
            self.lsp.health.update(|h| {
                h.insert(id.to_string(), ServerHealth::GaveUp { crashes: restarts });
            });
            let msg = format!(
                "{id} crashed {restarts} times and was not restarted — click it in the status bar to try again"
            );
            eprintln!("e: {msg}");
            Self::notify(&msg);
            return;
        }
        eprintln!("e: {id} exited unexpectedly — restarting ({restarts}/{MAX_LSP_RESTARTS})");
        if self.restart_lsp(id, restarts) {
            Self::notify(&format!("{id} crashed and was restarted"));
        }
    }

    /// Start server `id` afresh and hand it every open document it serves.
    /// Returns whether it came up.
    pub(crate) fn restart_lsp(&self, id: &str, restarts: u32) -> bool {
        let laravel = self.is_laravel_project();
        let buffers = self.editor.buffers.get_untracked();
        // Which spec? Any open buffer whose language wants this server says.
        // With none open there is nothing to serve; the next open starts it.
        let Some(spec) = buffers
            .iter()
            .find_map(|b| lsp_registry::spec_for(b.file.language, laravel, id))
        else {
            self.lsp.health.update(|h| {
                h.remove(id);
            });
            return false;
        };
        let Some(client) = self.start_lsp(&spec, restarts) else {
            return false;
        };
        for b in &buffers {
            let Some(s) = lsp_registry::spec_for(b.file.language, laravel, id) else {
                continue;
            };
            if let Some(uri) = b.uri.as_ref() {
                client.did_open(
                    uri,
                    s.language_id,
                    b.lsp_version.get_untracked(),
                    &b.doc.text().to_string(),
                );
            }
        }
        true
    }

    /// Status-bar click: whatever is down for the active file comes back. A
    /// server that isn't installed gets installed (its command runs in a
    /// terminal tab, and it starts when the binary appears); a crashed or
    /// failed one is simply tried again.
    pub fn retry_lsp_for_active(&self) {
        let laravel = self.is_laravel_project();
        let mut specs = self
            .active_buffer()
            .map(|b| lsp_registry::server_specs(b.file.language, laravel))
            .unwrap_or_default();
        // Same fallback as the status line: the PHP servers, in a Laravel project.
        if specs.is_empty() && laravel {
            specs = lsp_registry::server_specs(Language::Php, laravel);
        }
        for spec in specs {
            let running = self
                .lsp
                .clients
                .with_untracked(|m| m.get(spec.id).map(|c| c.is_alive()))
                .unwrap_or(false);
            if running {
                continue;
            }
            let missing = matches!(
                self.lsp.health.with_untracked(|h| h.get(spec.id).cloned()),
                Some(ServerHealth::NotInstalled)
            ) && !program_on_path(spec.program);
            if missing {
                self.install_lsp(&spec);
                continue;
            }
            self.lsp.health.update(|h| {
                h.remove(spec.id);
            });
            self.restart_lsp(spec.id, 0);
        }
    }

    /// Command palette: install (or start) the server with `id`. Installed and
    /// running → say so; installed but down → start it; missing → install it.
    pub fn install_lsp_by_id(&self, id: &str) {
        let laravel = true; // the palette entries are the Laravel servers
        let Some(spec) = lsp_registry::server_specs(Language::Php, laravel)
            .into_iter()
            .chain(lsp_registry::server_specs(Language::Blade, laravel))
            .find(|s| s.id == id)
        else {
            return;
        };
        let running = self
            .lsp
            .clients
            .with_untracked(|m| m.get(spec.id).map(|c| c.is_alive()))
            .unwrap_or(false);
        if running {
            Self::notify(&format!("{} is installed and running", spec.id));
            return;
        }
        if program_on_path(spec.program) {
            self.lsp.health.update(|h| {
                h.remove(spec.id);
            });
            if self.restart_lsp(spec.id, 0) {
                Self::notify(&format!("{} is installed — started it", spec.id));
            } else {
                Self::notify(&format!(
                    "{} is installed — it starts with the next PHP file you open",
                    spec.id
                ));
            }
            return;
        }
        self.install_lsp(&spec);
    }

    /// Run a server's install command in a terminal tab, then watch `PATH` for
    /// the binary and start the server the moment it shows up — so "install
    /// intelephense" is one click, with no restart and nothing to type.
    pub fn install_lsp(&self, spec: &lsp_registry::ServerSpec) {
        let already = self.lsp_installing.with_untracked(|s| s.contains(spec.id));
        if already {
            Self::notify(&format!("{} is still installing", spec.id));
            return;
        }
        self.lsp_installing.update(|s| {
            s.insert(spec.id.to_string());
        });
        eprintln!("e: installing {} with `{}`", spec.id, spec.install);
        self.run_task(&format!("install {}", spec.id), spec.install);
        self.watch_for_installed(*spec, 0);
    }

    /// Poll for `spec.program` on `PATH` every two seconds, for up to ten
    /// minutes (a `composer global require` on a cold cache can take a while).
    pub(crate) fn watch_for_installed(&self, spec: lsp_registry::ServerSpec, attempt: u32) {
        const EVERY: std::time::Duration = std::time::Duration::from_secs(2);
        const MAX_ATTEMPTS: u32 = 300;
        let app = *self;
        floem::action::exec_after(EVERY, move |_| {
            if program_on_path(spec.program) {
                app.lsp_installing.update(|s| {
                    s.remove(spec.id);
                });
                app.lsp.health.update(|h| {
                    h.remove(spec.id);
                });
                eprintln!("e: {} installed — starting it", spec.id);
                if app.restart_lsp(spec.id, 0) {
                    Self::notify(&format!("{} installed and started", spec.id));
                } else {
                    Self::notify(&format!(
                        "{} installed — it starts with the next {} file you open",
                        spec.id, spec.language_id
                    ));
                }
                return;
            }
            if attempt + 1 >= MAX_ATTEMPTS {
                app.lsp_installing.update(|s| {
                    s.remove(spec.id);
                });
                eprintln!("e: gave up waiting for {} to appear on PATH", spec.id);
                return;
            }
            app.watch_for_installed(spec, attempt + 1);
        });
    }

    /// One line of server health for the active file, and whether all is well:
    /// `intelephense ✓ · laravel-lsp ↓`, or what a server is busy with.
    /// Reactive: reads the health, progress and client signals.
    pub fn lsp_status(&self) -> Option<(String, bool)> {
        let laravel = self.is_laravel_project();
        let mut specs = self
            .active_buffer()
            .map(|b| lsp_registry::server_specs(b.file.language, laravel))
            .unwrap_or_default();
        // In a Laravel project the PHP servers matter whatever file is open —
        // a missing one should be visible before the first PHP file is.
        if specs.is_empty() && laravel {
            specs = lsp_registry::server_specs(Language::Php, laravel);
        }
        if specs.is_empty() {
            return None;
        }
        let health = self.lsp.health.get();
        let progress = self.lsp.progress.get();
        let alive = self.lsp.clients.with(|m| {
            m.iter()
                .map(|(k, c)| (k.clone(), c.is_alive()))
                .collect::<HashMap<_, _>>()
        });
        let mut parts = Vec::new();
        let mut all_ok = true;
        for spec in specs {
            let up = alive.get(spec.id).copied().unwrap_or(false);
            let text = match (up, health.get(spec.id)) {
                (true, Some(ServerHealth::Starting { .. })) => format!("{} …", spec.id),
                (true, _) => match progress.get(spec.id) {
                    Some(p) => format!("{}: {p}", spec.id),
                    None => format!("{} ✓", spec.id),
                },
                (false, Some(ServerHealth::NotInstalled)) => {
                    if self.lsp_installing.with(|s| s.contains(spec.id)) {
                        format!("{} ↓ installing…", spec.id)
                    } else {
                        all_ok = false;
                        format!("{} ↓ click to install", spec.id)
                    }
                }
                (false, Some(ServerHealth::GaveUp { .. })) => {
                    all_ok = false;
                    format!("{} ✗ crashed", spec.id)
                }
                (false, Some(ServerHealth::Failed(_))) => {
                    all_ok = false;
                    format!("{} ✗ failed", spec.id)
                }
                // Never started (no file of this language opened yet), or
                // between a crash and its restart.
                (false, _) => continue,
            };
            parts.push(text);
        }
        if parts.is_empty() {
            return None;
        }
        Some((parts.join(" · "), all_ok))
    }

    /// Apply per-URI edits: open buffers through the document (undoable, and the
    /// editor stays in sync), everything else straight to disk. Returns the
    /// number of files changed.
    pub fn apply_workspace_edits(&self, edits: &[(String, Vec<TextEdit>)]) -> usize {
        let buffers = self.editor.buffers.get_untracked();
        let mut changed = 0usize;
        for (uri, list) in edits {
            if list.is_empty() {
                continue;
            }
            let open = buffers
                .iter()
                .find(|b| b.uri.as_deref() == Some(uri.as_str()));
            match open {
                Some(buf) => {
                    if Self::apply_edits_to_buffer(buf, list) {
                        changed += 1;
                    }
                }
                None => {
                    let path = uri_to_path(uri);
                    let text = match std::fs::read_to_string(&path) {
                        Ok(t) => t,
                        // A file the edit brings into being (a missing view, a
                        // new lang file): start from nothing.
                        Err(_) if !path.exists() => {
                            if let Some(dir) = path.parent() {
                                let _ = std::fs::create_dir_all(dir);
                            }
                            String::new()
                        }
                        Err(_) => {
                            eprintln!("e: workspace edit: could not read {}", path.display());
                            continue;
                        }
                    };
                    let updated = crate::rename_preview::apply_edits(&text, list);
                    if updated == text {
                        continue;
                    }
                    match std::fs::write(&path, updated) {
                        Ok(()) => changed += 1,
                        Err(e) => {
                            eprintln!("e: workspace edit: could not write {}: {e}", path.display())
                        }
                    }
                }
            }
        }
        if changed > 0 {
            self.fs_rev.update(|r| *r += 1);
            self.check_external_changes();
        }
        changed
    }

    /// Apply edits to one open buffer, last-first so earlier offsets stay valid.
    /// Returns whether anything changed.
    pub(crate) fn apply_edits_to_buffer(buf: &Buffer, edits: &[TextEdit]) -> bool {
        let Some(editor) = buf.editor.get_untracked() else {
            // No view yet: replace the whole text instead of editing in place.
            let text = buf.doc.text().to_string();
            let updated = crate::rename_preview::apply_edits(&text, edits);
            if updated == text {
                return false;
            }
            buf.doc
                .edit_single(Selection::region(0, text.len()), &updated, EditType::Other);
            return true;
        };
        let mut offs: Vec<(usize, usize, String)> = edits
            .iter()
            .map(|e| {
                let s = editor.offset_of_line_col(
                    e.range.start.line as usize,
                    e.range.start.character as usize,
                );
                let en = editor
                    .offset_of_line_col(e.range.end.line as usize, e.range.end.character as usize);
                (s, en.max(s), e.new_text.clone())
            })
            .collect();
        offs.retain(|(s, en, text)| !(s == en && text.is_empty()));
        if offs.is_empty() {
            return false;
        }
        apply_batch(&buf.doc, offs, EditType::InsertChars);
        true
    }

    /// Format the active buffer in place.
    ///
    /// A Laravel project's formatting is whatever Pint says it is — that is what
    /// CI enforces — so when the project ships Pint it wins over the language
    /// server, which formats PHP to its own taste and would fight it.
    pub fn format_active(&self) {
        self.format_active_then(|| {});
    }

    /// Format the active buffer, then run `then` on the UI thread — right away
    /// when there is nothing to format, or once the server's edits have been
    /// applied. The request itself runs off the UI thread; formatting a large
    /// file used to freeze the window for as long as the server took.
    pub fn format_active_then(&self, then: impl FnOnce() + 'static) {
        match self.active_buffer() {
            Some(buf) => self.format_buffer_then(buf, then),
            None => then(),
        }
    }

    /// [`Self::format_active_then`] for a given buffer, active or not.
    pub(crate) fn format_buffer_then(&self, buf: Buffer, then: impl FnOnce() + 'static) {
        // Pint, off the UI thread: it is a PHP process per save. Its result is
        // applied only if the text hasn't changed meanwhile — as one edit, so
        // it is a single undo step. When the project has Pint, Pint alone
        // decides: "nothing to change" must not hand the file to the
        // language server, which formats PHP to its own taste.
        let root = self.root.get_untracked();
        if let (Language::Php, Some(path)) = (buf.file.language, buf.file.path.clone()) {
            if crate::phptools::has_pint(&root) {
                let text = buf.doc.text().to_string();
                let rev = buf.doc.rev();
                self.spawn_bg(
                    move || crate::phptools::pint_format(&root, &path, &text),
                    move |formatted: Option<String>| {
                        if let Some(formatted) = formatted {
                            if buf.doc.rev() == rev {
                                let end = buf.doc.text().len();
                                buf.doc.edit_single(
                                    Selection::region(0, end),
                                    &formatted,
                                    EditType::Other,
                                );
                            } else {
                                Self::notify("Formatting skipped: the file changed meanwhile");
                            }
                        }
                        then();
                    },
                );
                return;
            }
        }
        if self.lsp_language_id(buf.file.language).is_none() {
            then();
            return;
        }
        let (Some(client), Some(uri)) = (self.lsp_for_language(buf.file.language), buf.uri.clone())
        else {
            then();
            return;
        };
        // If the text changes while the server is thinking, its edits describe
        // a document that no longer exists; skip rather than corrupt.
        let version = buf.lsp_version.get_untracked();
        self.spawn_bg(
            move || client.formatting(&uri, 4, true).ok(),
            move |edits: Option<Vec<TextEdit>>| {
                if let Some(edits) = edits.filter(|e| !e.is_empty()) {
                    if buf.lsp_version.get_untracked() == version {
                        Self::apply_edits_to_buffer(&buf, &edits);
                    } else {
                        eprintln!("e: format: the file changed while formatting; skipped");
                        Self::notify("Formatting skipped: the file changed meanwhile");
                    }
                }
                then();
            },
        );
    }

    /// Request LSP code actions (quick fixes / refactors like extract) at the
    /// cursor or selection, and open the picker.
    pub fn request_code_actions(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let clients = self.lsp_all_for_active();
        let (Some(uri), Some(editor)) = (buf.uri.clone(), buf.editor.get_untracked()) else {
            return;
        };
        let cursor = editor.cursor.get_untracked();
        let (sl, sc, el, ec) = if let CursorMode::Insert(sel) = cursor.mode.clone() {
            match sel.regions().first() {
                Some(r) => {
                    let (al, ac) = editor.offset_to_line_col(r.min());
                    let (bl, bc) = editor.offset_to_line_col(r.max());
                    (al as u32, ac as u32, bl as u32, bc as u32)
                }
                None => (0, 0, 0, 0),
            }
        } else {
            let (l, c) = editor.offset_to_line_col(cursor.offset());
            (l as u32, c as u32, l as u32, c as u32)
        };

        // `e`'s own Laravel refactorings at the caret — cheap text transforms,
        // so they're computed right here and shown first.
        let text = buf.doc.text().to_string();
        let mut local = self.lint_fix_actions(&uri, sl, sc);
        // On a form field: carry it through the stack.
        if let Some(f) = crate::contract_heal::field_here(&text, buf.file.language, cursor.offset())
        {
            local.push(e_lsp::CodeActionItem {
                title: format!(
                    "Carry “{}” through the stack (rules, model, column)",
                    f.field
                ),
                edits: Vec::new(),
                command: Some(serde_json::json!({ "e": "heal" })),
                server: "e".to_string(),
            });
        }
        // On a line the running app threw from: the exception, or the
        // failed job.
        if let Some(c) = buf
            .file
            .path
            .as_ref()
            .and_then(|p| self.caught_at(p, sl as usize))
        {
            let item = |title: String, action: &str| e_lsp::CodeActionItem {
                title,
                edits: Vec::new(),
                command: Some(serde_json::json!({ "e": action, "line": c.line })),
                server: "e".to_string(),
            };
            match &c.job {
                Some(job) => {
                    let short = job.class.rsplit('\\').next().unwrap_or(&job.class);
                    let what = match job.retry_ids.len() {
                        1 => short.to_string(),
                        n => format!("{short} ({n} failures)"),
                    };
                    local.push(item(
                        format!("Failed job: retry {what} (queue:retry)"),
                        "job.retry",
                    ));
                    local.push(item(
                        format!("Failed job: ask the agent to fix {}", c.summary()),
                        "job.fix",
                    ));
                    local.push(item("Failed job: dismiss".to_string(), "caught.dismiss"));
                }
                None => {
                    local.push(item(
                        format!("Exception: ask the agent to fix {}", c.summary()),
                        "caught.fix",
                    ));
                    local.push(item("Exception: dismiss".to_string(), "caught.dismiss"));
                }
            }
        }
        // On a line with an evaluated expression: all of it, or away.
        if self.eval_at(sl as usize).is_some() {
            let item = |title: &str, action: &str| e_lsp::CodeActionItem {
                title: title.to_string(),
                edits: Vec::new(),
                command: Some(serde_json::json!({ "e": action, "line": sl })),
                server: "e".to_string(),
            };
            if let Some(advice) = self.eval_at(sl as usize).and_then(|e| e.advice) {
                local.push(item(
                    &format!(
                        "Explain: add an index on {} (new migration)",
                        advice.describe()
                    ),
                    "eval.index",
                ));
            }
            local.push(item(
                "Evaluate: show the full result and its queries",
                "eval.show",
            ));
            local.push(item("Evaluate: clear the result", "eval.clear"));
        }
        // On a line with a runtime lens: the request behind it.
        let lens = buf.file.path.as_ref().and_then(|p| {
            let line = text.lines().nth(sl as usize)?;
            self.runtime_lens_at(p, line)
        });
        if buf
            .file
            .path
            .as_ref()
            .is_some_and(|p| self.migration_dry_run_offered(p))
        {
            local.push(e_lsp::CodeActionItem {
                title: "Migration: dry-run up() and down(), rolled back".to_string(),
                edits: Vec::new(),
                command: Some(serde_json::json!({ "e": "migration.dry_run" })),
                server: "e".to_string(),
            });
        }
        if let Some(lens) = lens {
            let (method, uri) = self
                .runtime
                .reqs
                .with_untracked(|l| {
                    l.iter()
                        .find(|r| r.id == lens.req_id)
                        .map(|r| (r.method.clone(), r.uri.clone()))
                })
                .unwrap_or_default();
            let item = |title: String, action: &str| e_lsp::CodeActionItem {
                title,
                edits: Vec::new(),
                command: Some(serde_json::json!({ "e": action, "id": lens.req_id })),
                server: "e".to_string(),
            };
            local.push(item(
                format!("Runtime: show {method} {uri}"),
                "runtime.open",
            ));
            local.push(item(
                format!("Runtime: ask the agent to fix {method} {uri} and measure it"),
                "runtime.fix",
            ));
            local.push(item(
                format!("Runtime: write a test from {method} {uri}"),
                "runtime.test",
            ));
        }
        local.extend(local_code_actions(
            &text,
            buf.file.language,
            cursor.offset(),
            &uri,
            &self.root.get_untracked(),
        ));

        if clients.is_empty() {
            if local.is_empty() {
                Self::notify("No code actions here");
                return;
            }
            self.problems.code_actions.set(local);
            self.problems.code_actions_open.set(true);
            return;
        }
        let diags = self
            .problems
            .by_file
            .with_untracked(|m| m.get(&uri).cloned().unwrap_or_default());
        let list_sig = self.problems.code_actions;
        let open_sig = self.problems.code_actions_open;
        self.spawn_bg(
            move || {
                // Offer every server's fixes together (intelephense quick fixes
                // plus laravel-lsp's framework fixes), asked in parallel.
                let mut list = local;
                list.extend(
                    fan_out(&clients, |client| {
                        client
                            .code_actions(&uri, sl, sc, el, ec, &diags)
                            .unwrap_or_default()
                    })
                    .into_iter()
                    .flatten(),
                );
                list
            },
            move |list: Vec<e_lsp::CodeActionItem>| {
                if list.is_empty() {
                    Self::notify("No code actions here");
                    return;
                }
                list_sig.set(list);
                open_sig.set(true);
            },
        );
    }

    /// Apply the chosen code action: its edits if it carries them, otherwise run
    /// its command and let the server send the edits back as `workspace/applyEdit`.
    pub fn apply_code_action(&self, index: usize) {
        self.problems.code_actions_open.set(false);
        let Some(item) = self
            .problems
            .code_actions
            .with_untracked(|l| l.get(index).cloned())
        else {
            return;
        };
        if !item.edits.is_empty() {
            // A file the action creates is worth looking at afterwards.
            let created: Vec<PathBuf> = item
                .edits
                .iter()
                .map(|(u, _)| uri_to_path(u))
                .filter(|p| !p.exists())
                .collect();
            self.apply_workspace_edits(&item.edits);
            if let Some(p) = created.into_iter().find(|p| p.exists()) {
                self.open_path(p);
            }
            return;
        }
        let Some(command) = item.command.clone() else {
            return;
        };
        // `e`'s own actions carry what to do rather than an LSP command.
        if item.server == "e" {
            let id = command.get("id").and_then(|v| v.as_str()).unwrap_or("");
            match command.get("e").and_then(|v| v.as_str()) {
                Some("runtime.open") => self.runtime_lens_open(id),
                Some(action @ ("eval.show" | "eval.clear" | "eval.index")) => {
                    let line = command.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                    match action {
                        "eval.show" => self.show_eval(line),
                        "eval.index" => self.add_advised_index(line),
                        _ => self.clear_eval(line),
                    }
                }
                Some("runtime.fix") => self.runtime_fix(id),
                Some("runtime.test") => self.runtime_test(id),
                Some(action @ ("caught.fix" | "caught.dismiss" | "job.retry" | "job.fix")) => {
                    let line = command.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                    if let Some(path) = self.active_buffer().and_then(|b| b.file.path.clone()) {
                        match action {
                            "caught.fix" => self.caught_to_agent(&path, line),
                            "job.retry" => self.retry_failed_job(&path, line),
                            "job.fix" => self.failed_job_to_agent(&path, line),
                            _ => self.dismiss_caught(&path, line),
                        }
                    }
                }
                Some("migration.dry_run") => self.migration_dry_run(),
                Some("heal") => self.heal_field(),
                _ => {}
            }
            return;
        }
        let client = self
            .lsp
            .clients
            .with_untracked(|m| m.values().find(|c| c.name() == item.server).cloned());
        let Some(client) = client else {
            Self::notify("That code action's language server is no longer running");
            return;
        };
        let title = item.title.clone();
        self.spawn_bg(
            move || {
                client
                    .execute_command(&command)
                    .map_err(|e| format!("{e:#}"))
            },
            move |result: Result<serde_json::Value, String>| {
                if let Err(e) = result {
                    eprintln!("e: code action `{title}` failed: {e}");
                    Self::notify(&format!("Code action failed: {e}"));
                }
            },
        );
    }
}
