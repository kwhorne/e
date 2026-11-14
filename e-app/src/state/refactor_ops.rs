//! Getting around and reshaping the code: Livewire and Inertia, moving a class, renaming through the language server.

use super::*;

impl AppState {
    // ---- Livewire ------------------------------------------------------

    /// Completion items for a `wire:model` value, from the component's class.
    pub(crate) fn livewire_property_items(
        &self,
        buf: &Buffer,
        partial: &str,
    ) -> Option<Vec<lsp_types::CompletionItem>> {
        let path = buf.file.path.as_ref()?;
        let comp = crate::livewire::resolve(&self.root.get_untracked(), path)?;
        let src = self.livewire_class_source(&comp)?;
        let lower = partial.to_lowercase();
        let items: Vec<lsp_types::CompletionItem> = crate::livewire::properties(&src)
            .into_iter()
            .filter(|p| lower.is_empty() || p.to_lowercase().starts_with(&lower))
            .map(|p| lsp_types::CompletionItem {
                label: p.clone(),
                insert_text: Some(p.clone()),
                kind: Some(lsp_types::CompletionItemKind::FIELD),
                detail: Some("Livewire property".to_string()),
                ..Default::default()
            })
            .collect();
        if items.is_empty() {
            None
        } else {
            Some(items)
        }
    }

    /// A component's class source: the open buffer's text when it is being
    /// edited (a single-file component usually is), else the file on disk.
    pub(crate) fn livewire_class_source(
        &self,
        comp: &crate::livewire::Component,
    ) -> Option<String> {
        let open = self.editor.buffers.with_untracked(|bs| {
            bs.iter()
                .find(|b| b.file.path.as_deref() == Some(comp.class_file.as_path()))
                .map(|b| b.doc.text().to_string())
        });
        open.or_else(|| std::fs::read_to_string(&comp.class_file).ok())
    }

    /// Caret on a `<livewire:name>`, `@livewire('name')` or
    /// `Route::livewire(…, 'name')` jumps to that component.
    pub(crate) fn goto_livewire_component(&self) -> bool {
        let Some(buf) = self.active_buffer() else {
            return false;
        };
        if !matches!(buf.file.language, Language::Blade | Language::Php) {
            return false;
        }
        let Some(editor) = buf.editor.get_untracked() else {
            return false;
        };
        let text = buf.doc.text().to_string();
        let offset = editor.cursor.get_untracked().offset();
        let Some(name) = crate::livewire::component_at(&text, offset) else {
            return false;
        };
        match crate::livewire::resolve_name(&self.root.get_untracked(), &name) {
            Some(path) => {
                self.jump_to(&path_to_uri(&path), 0, 0);
                true
            }
            None => false,
        }
    }

    /// Caret on an `Inertia::render('Page')` string jumps to the page component.
    pub(crate) fn goto_inertia_page(&self) -> bool {
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
        let Some(name) = crate::inertia::render_at(&text, offset) else {
            return false;
        };
        if let Some(p) = crate::inertia::resolve_page(&self.root.get_untracked(), &name) {
            self.jump_to(&path_to_uri(&p), 0, 0);
            true
        } else {
            false
        }
    }

    /// Open an Inertia page component if `name` resolves to one, else fall back
    /// to Blade view resolution. Used by the architecture map.
    pub fn open_page_or_view(&self, name: &str) {
        let root = self.root.get_untracked();
        if let Some(p) = crate::inertia::resolve_page(&root, name) {
            self.jump_to(&path_to_uri(&p), 0, 0);
            return;
        }
        if let Some(data) = self.project.laravel.get_untracked() {
            if let Some((p, l, c)) = laravel::navigate(&data, laravel::Helper::View, name) {
                self.jump_to(&path_to_uri(&p), l, c);
            }
        }
    }

    /// Jump between a Livewire component's Blade view and its class file.
    pub fn livewire_companion(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(path) = buf.file.path.clone() else {
            return;
        };
        let Some(comp) = crate::livewire::resolve(&self.root.get_untracked(), &path) else {
            Self::notify("Not a Livewire component");
            return;
        };
        if comp.class_file == comp.view_file {
            Self::notify("A single-file component: its class and view are this file");
            return;
        }
        let target = if path == comp.class_file {
            comp.view_file
        } else {
            comp.class_file
        };
        self.open_path(target);
    }

    /// If the caret sits on a Livewire property in the view, jump to its
    /// declaration in the class. Returns `true` if it handled the jump.
    pub(crate) fn livewire_goto(&self) -> bool {
        let Some(buf) = self.active_buffer() else {
            return false;
        };
        let Some(path) = buf.file.path.clone() else {
            return false;
        };
        let Some(comp) = crate::livewire::resolve(&self.root.get_untracked(), &path) else {
            return false;
        };
        // Only jump view → class here (class → view is the companion command).
        if path != comp.view_file {
            return false;
        }
        let Some(editor) = buf.editor.get_untracked() else {
            return false;
        };
        let text = buf.doc.text().to_string();
        let offset = editor.cursor.get_untracked().offset();
        let word = word_at(&text, offset);
        let word = word.trim_start_matches('$');
        if word.is_empty() {
            return false;
        }
        let Some(src) = self.livewire_class_source(&comp) else {
            return false;
        };
        if !crate::livewire::properties(&src).iter().any(|p| p == word) {
            return false;
        }
        let line = crate::livewire::property_line(&src, word).unwrap_or(0);
        self.jump_to(&path_to_uri(&comp.class_file), line, 0);
        true
    }

    /// Rename a Livewire property across both the class and the view. Returns
    /// `true` if it handled the rename.
    pub(crate) fn livewire_rename(&self, old: &str, new: &str) -> bool {
        let Some(buf) = self.active_buffer() else {
            return false;
        };
        let Some(path) = buf.file.path.clone() else {
            return false;
        };
        let Some(comp) = crate::livewire::resolve(&self.root.get_untracked(), &path) else {
            return false;
        };
        let Some(class_src) = self.livewire_class_source(&comp) else {
            return false;
        };
        if !crate::livewire::properties(&class_src)
            .iter()
            .any(|p| p == old)
        {
            return false;
        }
        // One file holding both halves: one rewrite, or the second would
        // overwrite the first.
        if comp.class_file == comp.view_file {
            let ok = self.rewrite_file(
                &comp.class_file,
                crate::livewire::single_file_rename(&class_src, old, new),
            );
            if ok {
                Self::notify(&format!("Renamed Livewire property `{old}` → `{new}`"));
            }
            return ok;
        }
        // Rewrite both files (targeted so unrelated tokens are left alone).
        let new_class = crate::livewire::class_rename(&class_src, old, new);
        let mut ok = self.rewrite_file(&comp.class_file, new_class);
        if let Ok(view_src) = std::fs::read_to_string(&comp.view_file) {
            let new_view = crate::livewire::view_rename(&view_src, old, new);
            ok &= self.rewrite_file(&comp.view_file, new_view);
        }
        // Only claim success if every write actually landed; on failure
        // `rewrite_file` has already told the user what went wrong.
        if ok {
            Self::notify(&format!("Renamed Livewire property `{old}` → `{new}`"));
        }
        ok
    }

    /// Replace a file's contents, editing the open buffer (undoable) if it is
    /// open, otherwise writing to disk. Returns whether the change landed — a
    /// disk write can fail (full/read-only disk), and callers must not report
    /// success when it did.
    pub(crate) fn rewrite_file(&self, path: &std::path::Path, content: String) -> bool {
        let open = self.editor.buffers.with_untracked(|bs| {
            bs.iter()
                .find(|b| b.file.path.as_deref() == Some(path))
                .map(|b| (b.doc.clone(), b.dirty))
        });
        if let Some((doc, dirty)) = open {
            let len = doc.text().len();
            let mut it = std::iter::once((Selection::region(0, len), content.as_str()));
            doc.edit(&mut it, EditType::InsertChars);
            dirty.set(true);
            true
        } else {
            let ok = Self::write_or_notify(path, &content);
            self.fs_rev.update(|r| *r += 1);
            ok
        }
    }

    /// Run `work` on a background thread and deliver its result to `on_done` on
    /// the UI thread. Replaces the `create_ext_action` + `thread::spawn`
    /// boilerplate repeated across the app.
    pub(crate) fn spawn_bg<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> T + Send + 'static,
        on_done: impl FnOnce(T) + 'static,
    ) {
        let send = create_ext_action(self.cx, on_done);
        std::thread::spawn(move || send(work()));
    }

    /// Write user data to disk, surfacing failures as a notification instead of
    /// swallowing them. Returns whether the write succeeded.
    pub(crate) fn write_or_notify(path: &std::path::Path, content: &str) -> bool {
        match buffer::write(path, content) {
            Ok(()) => true,
            Err(e) => {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
                Self::notify(&format!("Could not write {name}: {e}"));
                false
            }
        }
    }

    /// The textual rename that predates the language-server path.
    ///
    /// Whole-word matches in the active buffer only, so it also renames inside
    /// strings and comments — which is why it is the fallback and not the
    /// default.
    pub(crate) fn rename_textually(&self, word: &str, new_name: &str) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let text = buf.doc.text().to_string();
        let occ = whole_word_occurrences(&text, word);
        if occ.is_empty() {
            return;
        }
        let edits: Vec<(Selection, String)> = occ
            .iter()
            .map(|(s, e)| (Selection::region(*s, *e), new_name.to_string()))
            .collect();
        let mut it = edits.iter().map(|(s, t)| (s.clone(), t.as_str()));
        buf.doc.edit(&mut it, EditType::InsertChars);
    }

    // ---- Move class ------------------------------------------------------

    /// The project's PSR-4 maps, from `composer.json`.
    pub(crate) fn psr4(&self) -> Vec<crate::move_class::Psr4Root> {
        let root = self.root.get_untracked();
        std::fs::read_to_string(root.join("composer.json"))
            .map(|t| crate::move_class::psr4_roots(&t))
            .unwrap_or_default()
    }

    /// Open the prompt for moving the active file's class.
    pub fn open_move_class(&self) {
        let roots = self.psr4();
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(path) = buf.file.path.clone() else {
            return;
        };
        let project = self.root.get_untracked();
        let Ok(rel) = path.strip_prefix(&project) else {
            Self::notify("that file is outside the project");
            return;
        };
        let Some(fqn) = crate::move_class::fqn_for(&roots, rel) else {
            Self::notify("no PSR-4 mapping covers that file");
            return;
        };
        let r = self.rename;
        r.word.set(fqn.clone());
        r.new_name.set(fqn);
        self.rename.is_move.set(true);
        r.open.set(true);
    }

    /// Work out what moving the active class would do, and show it.
    pub fn plan_move_class(&self, new_fqn: &str) {
        self.rename.is_move.set(false);
        let roots = self.psr4();
        let old_fqn = self.rename.word.get_untracked();
        if new_fqn.trim().is_empty() || new_fqn == old_fqn {
            return;
        }
        let project = self.root.get_untracked();
        // The same walker search uses, so ignore rules apply and `vendor/`
        // never reaches the planner.
        let files: Vec<std::path::PathBuf> = crate::workspace_search::search(
            std::slice::from_ref(&project),
            crate::move_class::class_of(&old_fqn),
            Default::default(),
            5000,
            &crate::workspace_search::OpenTexts::new(),
            &|| false,
        )
        .into_iter()
        .filter_map(|h| h.path.strip_prefix(&project).ok().map(|p| p.to_path_buf()))
        .filter(|p| p.extension().is_some_and(|e| e == "php"))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();

        let app = *self;
        let plan = crate::move_class::plan_move(&roots, &old_fqn, new_fqn, &files, |p| {
            let abs = project.join(p);
            app.editor
                .buffers
                .with_untracked(|bs| {
                    bs.iter()
                        .find(|b| b.file.path.as_deref() == Some(abs.as_path()))
                        .map(|b| b.doc.text().to_string())
                })
                .or_else(|| std::fs::read_to_string(&abs).ok())
        });
        match plan {
            Some(p) => self.rename.move_plan.set(Some(p)),
            None => Self::notify("that move has no PSR-4 destination"),
        }
    }

    /// Carry out a planned move: rewrite the referrers, then move the file.
    pub fn confirm_move_class(&self) {
        let Some(plan) = self.rename.move_plan.get_untracked() else {
            return;
        };
        self.rename.move_plan.set(None);
        let project = self.root.get_untracked();
        let mut failed: Vec<String> = Vec::new();

        for r in plan.referrers.iter() {
            let abs = project.join(&r.path);
            if std::fs::write(&abs, &r.updated).is_err() {
                failed.push(r.path.display().to_string());
            }
        }
        // The moved file last: if a referrer write failed, the class is at least
        // still where every reference expects it.
        if let Some(moved) = &plan.moved {
            let to = project.join(&plan.to);
            if let Some(dir) = to.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if std::fs::write(&to, &moved.updated).is_err() {
                failed.push(plan.to.display().to_string());
            } else {
                let _ = std::fs::remove_file(project.join(&plan.from));
            }
        }

        self.fs_rev.update(|r| *r += 1);
        self.check_external_changes();
        if failed.is_empty() {
            Self::notify(&format!(
                "moved to {} ({})",
                plan.to.display(),
                plan.summary()
            ));
        } else {
            Self::notify(&format!("move could not write: {}", failed.join(", ")));
        }
    }

    pub fn cancel_move_class(&self) {
        self.rename.move_plan.set(None);
    }

    /// Ask the language server what this rename would change, and show it.
    ///
    /// Returns whether it took over. When there is no server, or it declines,
    /// the caller falls back to the textual rename that was here before — worse,
    /// but better than nothing.
    pub(crate) fn plan_rename_via_lsp(&self, word: &str, new_name: &str) -> bool {
        let Some(buf) = self.active_buffer() else {
            return false;
        };
        let (Some(client), Some(uri), Some(editor)) = (
            self.lsp_for_active(),
            buf.uri.clone(),
            buf.editor.get_untracked(),
        ) else {
            return false;
        };
        let (line, col) = editor.offset_to_line_col(editor.cursor.get_untracked().offset());
        let (word, new_name) = (word.to_string(), new_name.to_string());
        let name_for_request = new_name.clone();
        let app = *self;

        self.rename.busy.set(true);
        let busy = self.rename.busy;
        let out = self.rename.plan;
        let send = create_ext_action(
            self.cx,
            move |edits: Option<Vec<(String, Vec<lsp_types::TextEdit>)>>| {
                busy.set(false);
                let Some(edits) = edits.filter(|e| !e.is_empty()) else {
                    // The server had nothing for us; fall back rather than
                    // leaving the user with a rename that silently did nothing.
                    app.rename_textually(&word, &new_name);
                    return;
                };
                let plan = crate::rename_preview::plan(&word, &new_name, &edits, |p| {
                    // Prefer the open buffer: renaming against a stale copy of a
                    // file the user has edited would preview one thing and write
                    // another.
                    app.editor
                        .buffers
                        .with_untracked(|bs| {
                            bs.iter()
                                .find(|b| b.file.path.as_deref() == Some(p))
                                .map(|b| b.doc.text().to_string())
                        })
                        .or_else(|| std::fs::read_to_string(p).ok())
                });
                if plan.is_empty() {
                    app.rename_textually(&word, &new_name);
                    return;
                }
                out.set(Some(plan));
            },
        );
        std::thread::spawn(move || {
            send(
                client
                    .rename(&uri, line as u32, col as u32, &name_for_request)
                    .ok(),
            );
        });
        true
    }

    /// Write a planned rename to every file it touches.
    pub fn confirm_rename(&self) {
        let Some(plan) = self.rename.plan.get_untracked() else {
            return;
        };
        self.rename.plan.set(None);
        let mut written = 0usize;
        let mut failed: Vec<String> = Vec::new();
        for file in &plan.files {
            // Open buffers go through the document, so the change is undoable
            // and the editor stays in sync; the rest are written to disk.
            let open = self.editor.buffers.with_untracked(|bs| {
                bs.iter()
                    .find(|b| b.file.path.as_deref() == Some(file.path.as_path()))
                    .cloned()
            });
            match open {
                Some(buf) => {
                    let text = buf.doc.text().to_string();
                    let updated = crate::rename_preview::apply_edits(&text, &file.edits);
                    if updated != text {
                        let len = text.len();
                        buf.doc
                            .edit_single(Selection::region(0, len), &updated, EditType::Other);
                        written += 1;
                    }
                }
                None => {
                    let Ok(text) = std::fs::read_to_string(&file.path) else {
                        failed.push(file.path.display().to_string());
                        continue;
                    };
                    let updated = crate::rename_preview::apply_edits(&text, &file.edits);
                    if updated != text && buffer::write(&file.path, &updated).is_err() {
                        failed.push(file.path.display().to_string());
                    } else {
                        written += 1;
                    }
                }
            }
        }
        // Then the files that move (a renamed view), after their own edits.
        for (from, to) in &plan.moves {
            // An open file moves with its buffer's text — the edits above may
            // have just landed there, unsaved — and reopens at its new path.
            let open = self.editor.buffers.with_untracked(|bs| {
                bs.iter()
                    .find(|b| b.file.path.as_deref() == Some(from.as_path()))
                    .map(|b| (b.id, b.doc.text().to_string()))
            });
            let moved = match &open {
                Some((_, text)) => buffer::write(to, text)
                    .map_err(|e| e.to_string())
                    .and_then(|_| std::fs::remove_file(from).map_err(|e| e.to_string())),
                None => to
                    .parent()
                    .map_or(Ok(()), std::fs::create_dir_all)
                    .and_then(|_| std::fs::rename(from, to))
                    .map_err(|e| e.to_string()),
            };
            match (moved, open) {
                (Err(e), _) => failed.push(format!("{} ({e})", from.display())),
                (Ok(()), Some((id, _))) => {
                    self.force_close(id);
                    self.open_path(to.clone());
                }
                (Ok(()), None) => {}
            }
        }
        // Then the files it creates (a heal's migration).
        failed.extend(Self::create_planned_files(&plan.creates));
        self.fs_rev.update(|r| *r += 1);
        self.check_external_changes();
        if !failed.is_empty() {
            Self::notify(&format!("could not write: {}", failed.join(", ")));
        }
        eprintln!("e: renamed in {written} file(s)");
    }

    pub fn cancel_rename(&self) {
        self.rename.plan.set(None);
    }

    pub fn apply_rename(&self) {
        let r = self.rename;
        if !r.open.get_untracked() {
            return;
        }
        let word = r.word.get_untracked();
        let new_name = r.new_name.get_untracked();
        r.open.set(false);
        if new_name.is_empty() || new_name == word {
            self.rename.is_move.set(false);
            return;
        }
        if self.rename.is_move.get_untracked() {
            self.plan_move_class(&new_name);
            return;
        }
        if let Some(helper) = r.laravel.get_untracked() {
            self.plan_laravel_rename(helper, word, new_name);
            return;
        }
        // Livewire property rename spans the class *and* the view.
        let prop = word.trim_start_matches('$');
        if self.livewire_rename(prop, new_name.trim_start_matches('$')) {
            return;
        }
        // Ask the language server first: it knows which occurrences are the
        // symbol and which are a word that happens to match inside a string or
        // a comment, and it sees the whole workspace rather than this buffer.
        if self.plan_rename_via_lsp(&word, &new_name) {
            return;
        }
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let text = buf.doc.text().to_string();
        let occ = whole_word_occurrences(&text, &word);
        if occ.is_empty() {
            return;
        }
        let edits: Vec<(Selection, String)> = occ
            .iter()
            .map(|(s, e)| (Selection::region(*s, *e), new_name.clone()))
            .collect();
        let mut it = edits.iter().map(|(s, t)| (s.clone(), t.as_str()));
        buf.doc.edit(&mut it, EditType::InsertChars);
    }

    /// Save all dirty buffers to disk (no formatting) — used by idle auto-save.
    pub fn maybe_autosave(&self) {
        if !self.settings.get_untracked().autosave {
            return;
        }
        let last = self.editor.last_edit.get_untracked();
        if last == 0 || now_ms().saturating_sub(last) < 1500 {
            return;
        }
        self.editor.last_edit.set(0);
        let buffers = self.editor.buffers.get_untracked();
        for b in &buffers {
            // A file that changed on disk under unsaved edits waits for the
            // user's call in the conflict banner; autosave never makes it.
            if !b.dirty.get_untracked() || b.disk_changed.get_untracked() {
                continue;
            }
            let Some(path) = b.file.path.as_ref() else {
                continue;
            };
            if Self::disk_changed_unnoticed(b) {
                continue;
            }
            let text = b.doc.text().to_string();
            match buffer::write_with_encoding(path, &text, &b.encoding.get_untracked()) {
                Ok(()) => {
                    b.mark_saved();
                    Self::refresh_disk_mtime(b);
                    if let Some(uri) = b.uri.as_ref() {
                        for client in self.lsp_clients_for(b.file.language) {
                            client.did_save(uri, &text);
                        }
                    }
                    self.request_inlay_hints(b.id);
                    self.check_migration(b.id);
                    self.tests_after_save(path, b.file.language);
                    self.clear_caught_in(path);
                }
                // Reported once per pause in typing; the edits stay unsaved
                // (and backed up in the session) until a write succeeds.
                Err(e) => {
                    eprintln!("e: autosave failed: {e:#}");
                    Self::notify(&format!("Could not save {}: {e:#}", b.file.display_name()));
                }
            }
        }
    }
}
