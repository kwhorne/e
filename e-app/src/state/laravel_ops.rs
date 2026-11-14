//! Laravel: its project data, Tinker, the undo tree and the Laravel overlays, code generation, request replay and string completions.

use super::*;

impl AppState {
    /// If the workspace is a Laravel project, scrape its data in the background.
    /// Called at startup, on **Laravel: Refresh Project Data**, when a file the
    /// data comes from is saved in `e`, and when the freshness check sees one
    /// change on disk (a `make:controller`, a `git checkout`). One scrape at a
    /// time; a change during a scrape queues exactly one more.
    pub fn load_laravel(&self) {
        if !self.settings.get_untracked().laravel {
            return;
        }
        let root = self.root.get();
        if !laravel::is_laravel(&root) {
            return;
        }
        if self.laravel_loading.get_untracked() {
            self.laravel_reload_pending.set(true);
            return;
        }
        self.laravel_loading.set(true);
        // We know what's coming: start the PHP and Blade servers now, so their
        // handshake overlaps with the user finding the first file to open
        // instead of following it. Spawning is milliseconds; nothing blocks.
        self.ensure_lsp(Language::Php);
        self.ensure_lsp(Language::Blade);
        let app = *self;
        self.spawn_bg(
            move || {
                let data = laravel::load(&root);
                let fp = laravel_fingerprint(&root);
                let rules = crate::ide_json::load(&root);
                (data, fp, rules)
            },
            move |(data, fp, rules): (LaravelData, u64, Vec<crate::ide_json::Rule>)| {
                eprintln!(
                    "e: loaded Laravel project data ({} routes, {} views, {} translations, {} ide.json rules)",
                    data.routes.len(),
                    data.views.len(),
                    data.translations.len(),
                    rules.len()
                );
                app.project.laravel.set(Some(Arc::new(data)));
                app.project.ide_rules.set(Arc::new(rules));
                app.laravel_fp.set(Some(fp));
                app.laravel_loading.set(false);
                // Squiggles for missing views/routes/keys reflect the new data.
                app.relint_all();
                if app.laravel_reload_pending.get_untracked() {
                    app.laravel_reload_pending.set(false);
                    app.load_laravel();
                }
            },
        );
    }

    /// Idle-tick check (every fourth tick, i.e. about every two seconds): has
    /// anything `LaravelData` is scraped from changed on disk? If so, reload.
    /// The fingerprint is a few hundred `stat`s, done off the UI thread.
    pub fn check_laravel_freshness(&self) {
        if self.project.laravel.get_untracked().is_none() || self.laravel_fp_busy.get_untracked() {
            return;
        }
        let tick = self.laravel_tick.get_untracked() + 1;
        self.laravel_tick.set(tick);
        if tick % 4 != 0 {
            return;
        }
        let root = self.root.get_untracked();
        let app = *self;
        self.laravel_fp_busy.set(true);
        self.spawn_bg(
            move || laravel_fingerprint(&root),
            move |fp: u64| {
                app.laravel_fp_busy.set(false);
                match app.laravel_fp.get_untracked() {
                    Some(prev) if prev != fp => {
                        eprintln!("e: Laravel project files changed on disk — refreshing");
                        app.laravel_fp.set(Some(fp));
                        app.load_laravel();
                    }
                    None => app.laravel_fp.set(Some(fp)),
                    _ => {}
                }
            },
        );
    }

    /// A file was saved in `e`: if it's one the Laravel data comes from, the
    /// data is stale right now — don't wait for the freshness tick.
    pub(crate) fn laravel_touch(&self, path: &std::path::Path) {
        // The watch on the project sees the save too, and reloads then.
        if self.project.laravel.get_untracked().is_none() || crate::fs_watch::active() {
            return;
        }
        let root = self.root.get_untracked();
        let Ok(rel) = path.strip_prefix(&root) else {
            return;
        };
        let rel = rel.to_string_lossy();
        let relevant = rel == ".env"
            || rel.starts_with("routes/")
            || rel.starts_with("config/")
            || rel.starts_with("lang/")
            || rel.starts_with("resources/lang/")
            || rel.starts_with("app/View/Components/");
        if relevant {
            self.load_laravel();
        }
    }

    /// Re-run the lints for every open buffer (after project data changed).
    pub(crate) fn relint_all(&self) {
        let ids: Vec<u64> = self
            .editor
            .buffers
            .with_untracked(|bs| bs.iter().map(|b| b.id).collect());
        for id in ids {
            self.refresh_lint(id);
        }
    }

    pub fn toggle_tinker(&self) {
        self.tinker.open.update(|o| *o = !*o);
    }

    pub fn toggle_laravel_map(&self) {
        if !self.views.map_open.get_untracked() && self.project.laravel.get_untracked().is_none() {
            self.load_laravel();
        }
        self.views.map_open.update(|o| *o = !*o);
    }

    /// Run the current editor selection in Tinker (or just open the panel).
    pub fn run_tinker_selection(&self) {
        if let Some(buf) = self.active_buffer() {
            if let Some(editor) = buf.editor.get_untracked() {
                let cursor = editor.cursor.get_untracked();
                if let CursorMode::Insert(sel) = &cursor.mode {
                    if let Some(r) = sel.regions().iter().find(|r| r.min() != r.max()) {
                        let text = buf.doc.text().to_string();
                        let code =
                            text[r.min().min(text.len())..r.max().min(text.len())].to_string();
                        self.run_tinker(code);
                        return;
                    }
                }
            }
        }
        self.tinker.open.set(true);
    }

    /// Run PHP through `php artisan tinker` in the project root, capturing output.
    pub fn run_tinker(&self, code: String) {
        let root = self.root.get_untracked();
        if !root.join("artisan").is_file() {
            self.tinker
                .output
                .set("Not a Laravel project (no artisan).".into());
            return;
        }
        self.tinker.open.set(true);
        self.tinker.running.set(true);
        self.tinker.output.set("Running…".into());
        let out_sig = self.tinker.output;
        let running = self.tinker.running;
        let send = create_ext_action(self.cx, move |text: String| {
            out_sig.set(text);
            running.set(false);
        });
        std::thread::spawn(move || {
            let text = match crate::phptools::tinker(&root, &code) {
                Ok(o) => {
                    let mut s = String::from_utf8_lossy(&o.stdout).to_string();
                    let err = String::from_utf8_lossy(&o.stderr);
                    if !err.trim().is_empty() {
                        s.push_str(&err);
                    }
                    if s.trim().is_empty() {
                        s = "(no output)".to_string();
                    }
                    s
                }
                Err(e) => format!("failed to run tinker: {e}"),
            };
            send(text);
        });
    }

    // ---- Undo tree -----------------------------------------------------

    pub fn toggle_undo_tree(&self) {
        self.editor.undo_open.update(|o| *o = !*o);
    }

    /// Replace the active buffer's whole text with `text` from the undo tree,
    /// suppressing re-recording of our own edit.
    pub(crate) fn undo_apply(&self, buf: &Buffer, text: &str) {
        // A very large file keeps no history tree (see the buffer's setup):
        // its empty root would replace the file with nothing.
        if buf.large {
            Self::notify("Undo history isn't kept for files over 1 MB; ⌘Z still undoes");
            return;
        }
        buf.undo_nav.set(true);
        let len = buf.doc.text().len();
        let mut it = std::iter::once((Selection::region(0, len), text));
        buf.doc.edit(&mut it, EditType::InsertChars);
        buf.undo_nav.set(false);
        buf.doc.cache_rev().update(|r| *r += 1);
        self.editor.undo_rev.update(|r| *r += 1);
        if let Some(p) = &buf.file.path {
            buf.undo.borrow().save(&undo_store_path(p));
        }
    }

    pub fn undo_tree_undo(&self) {
        if let Some(buf) = self.active_buffer() {
            let t = buf.undo.borrow_mut().undo();
            if let Some(text) = t {
                self.undo_apply(&buf, &text);
            }
        }
    }

    pub fn undo_tree_redo(&self) {
        if let Some(buf) = self.active_buffer() {
            let t = buf.undo.borrow_mut().redo();
            if let Some(text) = t {
                self.undo_apply(&buf, &text);
            }
        }
    }

    pub fn undo_tree_goto(&self, id: usize) {
        if let Some(buf) = self.active_buffer() {
            let t = buf.undo.borrow_mut().goto(id);
            if let Some(text) = t {
                self.undo_apply(&buf, &text);
            }
        }
    }

    // ---- Event dispatch graph ------------------------------------------

    pub fn toggle_event_graph(&self) {
        let open = !self.views.event_open.get_untracked();
        self.views.event_open.set(open);
        if open {
            let root = self.root.get_untracked();
            let sig = self.views.event_graph;
            let send =
                create_ext_action(self.cx, move |g: Vec<crate::events::EventNode>| sig.set(g));
            std::thread::spawn(move || send(crate::events::dispatch_map(&root)));
        }
    }

    /// Caret on a dispatched event class jumps to a listener.
    pub(crate) fn goto_event(&self) -> bool {
        let Some(buf) = self.active_buffer() else {
            return false;
        };
        if buf.file.language != Language::Php {
            return false;
        }
        let Some(editor) = buf.editor.get_untracked() else {
            return false;
        };
        let text = buf.doc.text().to_string();
        let offset = editor.cursor.get_untracked().offset();
        let word = word_at(&text, offset);
        if word.is_empty()
            || !word
                .chars()
                .next()
                .map(|c| c.is_uppercase())
                .unwrap_or(false)
        {
            return false;
        }
        // Only where the event is dispatched (`event(new OrderShipped`,
        // `OrderShipped::dispatch(`): on a `use` line or a type hint, go to
        // definition means the class, which the language server finds.
        let (start, end) = word_range(&text, offset);
        let constructed = text[..start].trim_end().ends_with("new")
            && text[..start]
                .rfind('(')
                .map(|p| text[..p].trim_end())
                .is_some_and(|call| {
                    ["event", "dispatch", "Event::dispatch", "broadcast"]
                        .iter()
                        .any(|f| call.ends_with(f))
                });
        let dispatched = constructed || text[end..].starts_with("::dispatch");
        if !dispatched {
            return false;
        }
        let root = self.root.get_untracked();
        if let Some(node) = crate::events::dispatch_map(&root)
            .into_iter()
            .find(|n| n.event == word)
        {
            if let Some((_, Some(file))) = node.listeners.into_iter().find(|(_, f)| f.is_some()) {
                self.jump_to(&path_to_uri(&file), 0, 0);
                return true;
            }
        }
        false
    }

    pub fn open_event_file(&self, path: PathBuf) {
        self.jump_to(&path_to_uri(&path), 0, 0);
    }

    // ---- Code generation -----------------------------------------------

    /// Generate an Eloquent model from the table currently open in the database
    /// panel — fillable, casts, and relationships from the live schema + FKs.
    pub fn generate_model_from_table(&self) {
        let Some(table) = self.db.result_table.get_untracked() else {
            Self::notify("Open a table in the database panel first");
            return;
        };
        if !crate::codegen::valid_table(&table) {
            return;
        }
        let root = self.root.get_untracked();
        let name = crate::codegen::model_name(&table);
        let file = root.join(format!("app/Models/{name}.php"));
        if file.is_file() {
            Self::notify(&format!("{name} already exists — opening it"));
            self.open_path(file);
            return;
        }
        let state = *self;
        let file2 = file.clone();
        let send = create_ext_action(self.cx, move |content: Option<String>| match content {
            Some(c) => {
                if let Some(dir) = file2.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                if std::fs::write(&file2, c).is_ok() {
                    state.open_path(file2.clone());
                    Self::notify(&format!("Generated {}", file2.display()));
                }
            }
            None => Self::notify("Could not read the table schema"),
        });
        std::thread::spawn(move || {
            let content = e_db::from_env(&root)
                .and_then(|cfg| e_db::connect(&cfg).ok())
                .and_then(|conn| {
                    let cols = e_db::columns(&conn, &table).ok()?;
                    let fks = e_db::foreign_keys(&conn).unwrap_or_default();
                    Some(crate::codegen::generate_model(&table, &cols, &fks))
                });
            send(content);
        });
    }

    // ---- Validation ----------------------------------------------------

    /// Generate `'field' => 'rules'` from the live schema and insert them at the
    /// cursor (table inferred from the active file's resource name).
    pub fn generate_validation_rules(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(path) = buf.file.path.clone() else {
            return;
        };
        let root = self.root.get_untracked();
        let Some(name) = crate::relatedfiles::resource_name(&path) else {
            return;
        };
        let table = crate::eloquent::model_table(&root, &name);
        let cols = self
            .db
            .schema_cache
            .with_untracked(|m| m.get(&table).cloned());
        let Some(cols) = cols.filter(|c| !c.is_empty()) else {
            Self::notify(&format!("No live schema for table `{table}`"));
            return;
        };
        let text = crate::validation::generate_rules(&table, &cols);
        let Some(editor) = buf.editor.get_untracked() else {
            return;
        };
        let offset = editor.cursor.get_untracked().offset();
        let mut it = std::iter::once((Selection::region(offset, offset), text.as_str()));
        buf.doc.edit(&mut it, EditType::InsertChars);
        buf.dirty.set(true);
        Self::notify(&format!("Inserted validation rules for `{table}`"));
    }

    // ---- Related files -------------------------------------------------

    /// Show the files related to the active file's resource (model, migration,
    /// factory, controller, test, …).
    pub fn show_related_files(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(path) = buf.file.path.clone() else {
            return;
        };
        let root = self.root.get_untracked();
        let Some(name) = crate::relatedfiles::resource_name(&path) else {
            return;
        };
        let mut items = crate::relatedfiles::related(&root, &name);
        items.retain(|(_, p)| *p != path);
        if items.is_empty() {
            Self::notify("No related files found");
            return;
        }
        self.views.related_items.set(items);
        self.views.related_open.set(true);
    }

    pub fn open_related(&self, path: PathBuf) {
        self.views.related_open.set(false);
        self.open_path(path);
    }

    // ---- Inertia props contract ----------------------------------------

    /// Reconcile the active page component with the controller that renders it.
    pub fn compute_contract(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(path) = buf.file.path.clone() else {
            return;
        };
        let root = self.root.get_untracked();
        let Some(page) = crate::contract::page_name_of(&root, &path) else {
            Self::notify("Open an Inertia page component first");
            return;
        };
        let src = buf.doc.text().to_string();
        let schema = self.db.schema_cache.get_untracked();
        let shared = crate::inertia::shared_props(&root);
        let routes: Vec<(String, String)> = self
            .project
            .laravel
            .get_untracked()
            .map(|d| {
                d.routes
                    .iter()
                    .map(|r| (r.name.clone(), r.action.clone()))
                    .collect()
            })
            .unwrap_or_default();
        self.views.contract_open.set(true);
        self.views.contract.set(None);
        let sig = self.views.contract;
        let send = create_ext_action(self.cx, move |c: Option<crate::contract::Contract>| {
            sig.set(c)
        });
        std::thread::spawn(move || {
            send(crate::contract::build(
                &root, &page, &src, &schema, &shared, &routes,
            ));
        });
    }

    /// Write TypeScript interfaces for the current contract and open them.
    pub fn generate_contract_ts(&self) {
        let Some(c) = self.views.contract.get_untracked() else {
            return;
        };
        let schema = self.db.schema_cache.get_untracked();
        let ts = crate::contract::generate_ts(&c, &schema);
        let root = self.root.get_untracked();
        let dir = root.join("resources/js/types");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join(format!("{}.d.ts", c.page.replace('/', "")));
        if std::fs::write(&file, ts).is_ok() {
            self.views.contract_open.set(false);
            self.open_path(file);
        }
    }

    // ---- Request replay ------------------------------------------------

    pub fn close_request(&self) {
        self.req.open.set(false);
    }

    /// The app's base URL: the `app_url` setting if set; else what Grove says
    /// the site is served as (real host, and `http://` when it has no HTTPS);
    /// else the `https://<folder>.test` convention.
    pub fn app_base(&self) -> String {
        let s = self.settings.get_untracked().app_url;
        if !s.trim().is_empty() {
            return s.trim().trim_end_matches('/').to_string();
        }
        if let Some(site) = self.grove_site() {
            return site.base_url();
        }
        let name = self
            .root
            .get_untracked()
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "app".into());
        format!("https://{name}.test")
    }

    /// Generate a Pest feature test from the last replayed request (URL, status,
    /// and key assertions derived from the actual response), open it, and hook
    /// it into the test-runner / TDD loop.
    pub fn generate_pest_test(&self) {
        let url = self.req.url.get_untracked();
        let status = self.req.status.get_untracked().unwrap_or(200);
        let body = self.req.body.get_untracked();
        let root = self.root.get_untracked();
        let path = url_path(&url);
        let name = pest_test_name(&path);
        let assertions = pest_assertions(status, &body);
        let content = format!(
            "<?php\n\nit('GET {path} responds {status}', function () {{\n    $response = $this->get('{path}');\n\n{assertions}}});\n"
        );
        let dir = root.join("tests").join("Feature");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join(format!("{name}ReplayTest.php"));
        if std::fs::write(&file, &content).is_ok() {
            self.close_request();
            self.open_path(file.clone());
            self.agent_log_push("pest", format!("generated {}", file.display()));
        }
    }

    /// Replay an HTTP request against the app for a route `uri`, showing the
    /// response and (via Clockwork, if installed) the SQL queries it ran.
    pub fn send_request(&self, uri: &str) {
        let root = self.root.get_untracked();
        let base = {
            let s = self.settings.get_untracked().app_url;
            if s.trim().is_empty() {
                let name = root
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "app".into());
                format!("https://{name}.test")
            } else {
                s.trim().trim_end_matches('/').to_string()
            }
        };
        let path = substitute_route_params(uri);
        let url = format!("{}/{}", base, path.trim_start_matches('/'));

        self.req.open.set(true);
        self.req.running.set(true);
        self.req.error.set(None);
        self.req.url.set(url.clone());
        self.req.status.set(None);
        self.req.body.set(String::new());
        self.req.queries.set(Vec::new());
        self.req.inertia.set(None);

        let state = *self;
        let send = create_ext_action(self.cx, move |r: RequestResult| {
            state.req.running.set(false);
            state.req.status.set(r.status);
            state.req.time.set(r.time);
            state.req.body.set(r.body);
            state.req.queries.set(r.queries);
            state.req.error.set(r.error);
            state.req.inertia.set(r.inertia);
        });
        std::thread::spawn(move || {
            send(do_http_request(&base, &url));
        });
    }

    pub fn agent_edit_cancel(&self) {
        if let Some(edit) = self.agent.edit.get_untracked() {
            self.agent.edit.set(None);
            let _ = edit
                .reply
                .send(serde_json::json!({"ok": true, "applied": 0, "cancelled": true}));
        }
    }

    /// Offer Laravel completions if the cursor is inside a helper string.
    /// Returns true when the context was handled (so we skip the LSP).
    pub(crate) fn try_laravel_completion(&self, buffer_id: u64) -> bool {
        let Some(buf) = self.buffer_by_id(buffer_id) else {
            return false;
        };
        if !matches!(buf.file.language, Language::Php | Language::Blade) {
            return false;
        }
        let Some(editor) = buf.editor.get_untracked() else {
            return false;
        };
        let cursor = editor.cursor.get_untracked();
        let offset = cursor.offset();
        let text = buf.doc.text().to_string();
        let upto = offset.min(text.len());
        let line_start = text[..upto].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let line_before = &text[line_start..upto];
        let root = self.root.get_untracked();

        // `[UserController::class, 'sh…']`: the controller's public methods.
        if buf.file.language == Language::Php {
            if let Some((class, partial)) = laravel::controller_action_context(line_before) {
                let items = laravel::controller_methods(&root, &text, &class)
                    .into_iter()
                    .filter(|m| m.starts_with(&partial))
                    .map(|m| lsp_types::CompletionItem {
                        label: m.clone(),
                        insert_text: Some(m),
                        kind: Some(lsp_types::CompletionItemKind::METHOD),
                        detail: Some(format!("{class} action")),
                        ..Default::default()
                    })
                    .collect();
                let start = offset - partial.len();
                return self.show_string_completions(&buf, &editor, buffer_id, start, items);
            }
        }

        if buf.file.language == Language::Blade {
            // `wire:click="sa…"`: the component class's action methods.
            if let Some(partial) = crate::livewire::wire_action_partial(line_before) {
                let methods = buf
                    .file
                    .path
                    .as_ref()
                    .and_then(|p| crate::livewire::resolve(&root, p))
                    .and_then(|c| self.livewire_class_source(&c))
                    .map(|src| crate::livewire::actions(&src))
                    .unwrap_or_default();
                let items = methods
                    .into_iter()
                    .filter(|m| m.starts_with(&partial))
                    .map(|m| lsp_types::CompletionItem {
                        label: m.clone(),
                        insert_text: Some(m),
                        kind: Some(lsp_types::CompletionItemKind::METHOD),
                        detail: Some("Livewire action".to_string()),
                        ..Default::default()
                    })
                    .collect::<Vec<_>>();
                if !items.is_empty() {
                    let start = offset - partial.len();
                    return self.show_string_completions(&buf, &editor, buffer_id, start, items);
                }
            }
            // `<livewire:adm…` / `@livewire('adm…`: component names.
            if let Some(partial) = crate::livewire::tag_partial(line_before) {
                let items = crate::livewire::component_names(&root)
                    .into_iter()
                    .filter(|n| n.contains(&partial))
                    .map(|n| lsp_types::CompletionItem {
                        label: n.clone(),
                        insert_text: Some(n),
                        kind: Some(lsp_types::CompletionItemKind::MODULE),
                        detail: Some("Livewire component".to_string()),
                        ..Default::default()
                    })
                    .collect::<Vec<_>>();
                if !items.is_empty() {
                    let start = offset - partial.len();
                    return self.show_string_completions(&buf, &editor, buffer_id, start, items);
                }
            }
        }

        // `<x-alert ty…`: the component's props as attributes.
        if buf.file.language == Language::Blade {
            if let Some((component, partial)) = laravel::component_attr_context(line_before) {
                let items = laravel::component_props(&root, &component)
                    .into_iter()
                    .filter(|p| p.starts_with(&partial))
                    .map(|p| lsp_types::CompletionItem {
                        label: p.clone(),
                        insert_text: Some(format!("{p}=\"\"")),
                        kind: Some(lsp_types::CompletionItemKind::PROPERTY),
                        detail: Some(format!("<x-{component}> prop")),
                        ..Default::default()
                    })
                    .collect::<Vec<_>>();
                if !items.is_empty() {
                    let start = offset - partial.len();
                    return self.show_string_completions(&buf, &editor, buffer_id, start, items);
                }
            }
        }

        let Some(data) = self.project.laravel.get() else {
            return false;
        };

        // Laravel's own helpers. When the official Laravel server is running it
        // owns these contexts — it's project-accurate, understands more of them
        // and is maintained upstream — so ours stay as the fallback.
        if !self.laravel_lsp_running() {
            if let Some((helper, prefix)) = laravel::detect_context(line_before) {
                let items = laravel::completions(&data, helper, &prefix);
                let start = offset - prefix.len();
                return self.show_string_completions(&buf, &editor, buffer_id, start, items);
            }
        }

        // What packages declare in their `ide.json` (`new Axis('…')` takes one
        // of these strings, `->rule('…')` takes a validation rule, …). No
        // server reads those, so they apply either way.
        let rules = self.project.ide_rules.get_untracked();
        if rules.is_empty() {
            return false;
        }
        let Some(site) = crate::ide_json::call_site(line_before) else {
            return false;
        };
        let items = crate::ide_json::complete(&rules, &site, &data, &root);
        if items.is_empty() {
            return false;
        }
        let start = offset - site.prefix.len();
        self.show_string_completions(&buf, &editor, buffer_id, start, items)
    }

    /// Show `items` as the completion popup for the string starting at `start`.
    /// Returns true so the caller knows the context was handled (and skips the
    /// language server) — an empty list closes the popup but still counts.
    pub(crate) fn show_string_completions(
        &self,
        buf: &Buffer,
        editor: &Editor,
        buffer_id: u64,
        start: usize,
        items: Vec<lsp_types::CompletionItem>,
    ) -> bool {
        let cursor = editor.cursor.get_untracked();
        let (_, below) = editor.points_of_offset(start, cursor.affinity);
        let vp = editor.viewport.get_untracked();
        let win = buf.win_origin.get_untracked();

        let comp = self.completion;
        comp.anchor
            .set(Point::new(win.x + below.x - vp.x0, win.y + below.y - vp.y0));
        comp.buffer_id.set(Some(buffer_id));
        comp.start_offset.set(start);
        if items.is_empty() {
            comp.open.set(false);
        } else {
            comp.items.set(items);
            comp.selected.set(0);
            comp.open.set(true);
        }
        true
    }

    /// The app's locale, for which `lang/<locale>/` a missing key belongs to.
    pub(crate) fn app_locale(&self) -> String {
        self.project
            .laravel
            .get_untracked()
            .and_then(|d| {
                d.envs
                    .iter()
                    .find(|kv| kv.key == "APP_LOCALE")
                    .map(|kv| kv.value.clone())
                    .filter(|v| !v.is_empty())
                    .or_else(|| {
                        d.configs
                            .iter()
                            .find(|kv| kv.key == "app.locale")
                            .map(|kv| kv.value.clone())
                            .filter(|v| !v.is_empty())
                    })
            })
            .unwrap_or_else(|| "en".to_string())
    }

    /// Code actions that fix `e`'s own Laravel diagnostics under the caret:
    /// create the missing view, add the missing translation key.
    pub(crate) fn lint_fix_actions(
        &self,
        uri: &str,
        line: u32,
        col: u32,
    ) -> Vec<e_lsp::CodeActionItem> {
        // The servers' and `e`'s own: those are kept on the buffer.
        let mut all: Vec<Diagnostic> = self
            .problems
            .by_file
            .with_untracked(|m| m.get(uri).cloned().unwrap_or_default());
        all.extend(self.own_diagnostics(uri));
        let at: Vec<Diagnostic> = all
            .into_iter()
            .filter(|d| d.source.as_deref() == Some("laravel"))
            .filter(|d| {
                let s = &d.range.start;
                let e = &d.range.end;
                (s.line < line || (s.line == line && s.character <= col))
                    && (e.line > line || (e.line == line && e.character >= col))
            })
            .collect();
        if at.is_empty() {
            return Vec::new();
        }
        let root = self.root.get_untracked();
        let locale = self.app_locale();
        let read = |p: &Path| std::fs::read_to_string(p).ok();
        let mut out: Vec<e_lsp::CodeActionItem> =
            crate::laravel_lint::fixes(&at, &root, &locale, &read)
                .into_iter()
                .map(|f| e_lsp::CodeActionItem {
                    title: f.title,
                    edits: vec![(path_to_uri(&f.path), f.edits)],
                    command: None,
                    server: "e".to_string(),
                })
                .collect();
        out.extend(self.env_fix_actions(uri, &at, &root, &read));
        out
    }

    /// `env()` outside `config/` under the caret: read it through `config()`.
    pub(crate) fn env_fix_actions(
        &self,
        uri: &str,
        at: &[Diagnostic],
        root: &Path,
        read: &dyn Fn(&Path) -> Option<String>,
    ) -> Vec<e_lsp::CodeActionItem> {
        use e_laravel::env_guard;
        let flagged = at.iter().any(|d| {
            matches!(&d.code, Some(lsp_types::NumberOrString::String(c)) if c.starts_with(env_guard::CODE))
        });
        if !flagged {
            return Vec::new();
        }
        let Some(buf) = self
            .editor
            .buffers
            .with_untracked(|bs| bs.iter().find(|b| b.uri.as_deref() == Some(uri)).cloned())
        else {
            return Vec::new();
        };
        let Some(path) = buf.file.path.clone() else {
            return Vec::new();
        };
        let text = buf.doc.text().to_string();
        let configs: Vec<(String, String)> = std::fs::read_dir(root.join("config"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                let name = p.file_name()?.to_str()?.strip_suffix(".php")?.to_string();
                Some((name, std::fs::read_to_string(&p).ok()?))
            })
            .collect();
        let starts: Vec<(u32, u32)> = at
            .iter()
            .map(|d| (d.range.start.line, d.range.start.character))
            .collect();
        env_guard::env_calls(&text)
            .iter()
            .filter(|call| {
                let before = &text[..call.start];
                let line = before.matches('\n').count() as u32;
                let col = (call.start - before.rfind('\n').map_or(0, |i| i + 1)) as u32;
                starts.contains(&(line, col))
            })
            .filter_map(|call| env_guard::fix(root, &path, &text, call, &configs, read))
            .map(|f| e_lsp::CodeActionItem {
                title: f.title,
                edits: f
                    .edits
                    .into_iter()
                    .map(|(p, edits)| (path_to_uri(&p), edits))
                    .collect(),
                command: None,
                server: "e".to_string(),
            })
            .collect()
    }
}
