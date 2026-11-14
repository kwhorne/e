//! Buffers: new and save-as, opening and closing, tabs and the session, the split, saving, and changes on disk.

use super::*;

impl AppState {
    // ---- External file changes -----------------------------------------

    /// Read and store the on-disk mtime for a buffer (after we write it, to
    /// avoid mistaking our own save for an external change).
    pub(crate) fn refresh_disk_mtime(buf: &Buffer) {
        let mtime = buf
            .file
            .path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok());
        buf.disk_mtime.set(mtime);
    }

    /// Poll open files for on-disk changes (called on the idle tick). Clean
    /// buffers are reloaded silently; dirty ones are flagged for the user.
    pub fn check_external_changes(&self) {
        // Snapshot (id, path, last-known mtime) cheaply on the UI thread, then
        // do the actual `stat` calls on a worker thread — those can block on
        // slow/network filesystems and must never stall the UI.
        let buffers = self.editor.buffers.get_untracked();
        let mut items: Vec<(u64, PathBuf, Option<std::time::SystemTime>)> = Vec::new();
        for b in &buffers {
            if let Some(path) = b.file.path.as_ref() {
                items.push((b.id, path.clone(), b.disk_mtime.get_untracked()));
            }
        }
        // One check at a time: on a hung network mount the `stat`s block, and
        // a new thread every tick would pile up behind them.
        static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if items.is_empty() || BUSY.swap(true, std::sync::atomic::Ordering::AcqRel) {
            return;
        }
        let state = *self;
        // (id, new mtime, is_first_observation)
        let send = create_ext_action(
            self.cx,
            move |changed: Vec<(u64, std::time::SystemTime, bool)>| {
                BUSY.store(false, std::sync::atomic::Ordering::Release);
                for (id, mtime, first) in changed {
                    let Some(b) = state.buffer_by_id(id) else {
                        continue;
                    };
                    b.disk_mtime.set(Some(mtime));
                    if first {
                        continue;
                    }
                    if b.dirty.get_untracked() {
                        b.disk_changed.set(true);
                    } else {
                        state.reload_buffer(&b);
                    }
                }
            },
        );
        std::thread::spawn(move || {
            let mut out = Vec::new();
            for (id, path, prev) in items {
                let Some(mtime) = std::fs::metadata(&path)
                    .ok()
                    .and_then(|m| m.modified().ok())
                else {
                    continue;
                };
                match prev {
                    None => out.push((id, mtime, true)),
                    Some(p) if p != mtime => out.push((id, mtime, false)),
                    _ => {}
                }
            }
            send(out);
        });
    }

    /// Reload a buffer's contents from disk, discarding any unsaved edits.
    pub(crate) fn reload_buffer(&self, buf: &Buffer) {
        let Some(path) = buf.file.path.as_ref() else {
            return;
        };
        // Honour the file's detected encoding (a non-UTF-8 file must not be
        // re-read as raw UTF-8 on external change).
        let Ok((content, encoding)) = buffer::read_with_encoding(path) else {
            return;
        };
        buf.encoding.set(encoding);
        if content == buf.doc.text().to_string() {
            buf.disk_changed.set(false);
            return;
        }
        let old_len = buf.doc.text().len();
        let mut it = std::iter::once((Selection::region(0, old_len), content.as_str()));
        buf.doc.edit(&mut it, EditType::InsertChars);
        buf.mark_saved();
        buf.disk_changed.set(false);
        Self::refresh_disk_mtime(buf);
    }

    /// Reload the active buffer from disk (used by the conflict banner).
    pub fn reload_active_from_disk(&self) {
        if let Some(buf) = self.active_buffer() {
            self.reload_buffer(&buf);
        }
    }

    /// Dismiss the disk-change conflict, keeping the in-memory version.
    pub fn keep_active_version(&self) {
        if let Some(buf) = self.active_buffer() {
            buf.disk_changed.set(false);
        }
    }

    // ---- New file / save as --------------------------------------------

    /// Create a new, empty, untitled buffer and focus it.
    pub fn new_untitled(&self) {
        self.new_untitled_with(String::new());
    }

    /// Create an untitled buffer seeded with `initial` content and focus it.
    pub fn new_untitled_with(&self, initial: String) {
        // In the app's scope, for the reason `open_path` gives.
        let app = *self;
        floem::reactive::with_scope(self.cx, move || app.new_untitled_in_app_scope(initial))
    }

    pub(crate) fn new_untitled_in_app_scope(&self, initial: String) {
        let id = self.next_id.get_untracked();
        self.next_id.set(id + 1);

        let highlights: Highlights =
            Rc::new(RefCell::new(highlight_lines(Language::PlainText, &initial)));
        let doc = Rc::new(TextDocument::new(self.cx, initial.clone()));
        doc.auto_indent.set(true);
        let dirty = RwSignal::new(!initial.is_empty());
        let undo = Rc::new(RefCell::new(e_core::undotree::UndoTree::new(&initial)));
        let undo_nav = Rc::new(std::cell::Cell::new(false));
        let editors = crate::per_pane::PerPane::new(id, self.panes(), None);

        {
            let app = *self;
            let doc2 = doc.clone();
            let highlights = highlights.clone();
            let undo = undo.clone();
            let undo_nav = undo_nav.clone();
            doc.clone().add_on_update(move |update| {
                crate::state::settle_carets(editors, &update, &doc2.text());
                // Only on the change: every subscriber (title, tab, status)
                // re-renders on a set, even to the same value.
                if !dirty.get_untracked() {
                    dirty.set(true);
                }
                app.editor.last_edit.set(now_ms());
                let text = doc2.text().to_string();
                *highlights.borrow_mut() = highlight_lines(Language::PlainText, &text);
                doc2.cache_rev().update(|r| *r += 1);
                // Record into the undo tree, then release the borrow *before*
                // bumping undo_rev: the signal update runs effects synchronously
                // (e.g. the undo-tree view re-reads `undo.borrow()`), so holding
                // the mutable borrow across it double-borrows and aborts. Note a
                // `borrow_mut()` temporary in an `if` condition lives until the
                // end of the whole `if`, hence the explicit `let`.
                let recorded =
                    !undo_nav.get() && undo.borrow_mut().record(&text, now_ms() as u64, 700);
                if recorded {
                    app.editor.undo_rev.update(|r| *r += 1);
                }
            });
        }

        let buf = Buffer {
            id,
            file: FileInfo::scratch(),
            doc,
            dirty,
            saved: Rc::new(RefCell::new(Rope::from(""))),
            highlights,
            diag_lines: Rc::new(RefCell::new(Vec::new())),
            git_marks: Rc::new(RefCell::new(Vec::new())),
            head_text: Rc::new(RefCell::new(None)),
            bp_marks: Rc::new(RefCell::new(Default::default())),
            stop_line: Rc::new(RefCell::new(None)),
            test_marks: Rc::new(RefCell::new(Default::default())),
            find_marks: Rc::new(RefCell::new(Vec::new())),
            bracket_marks: Rc::new(RefCell::new(Vec::new())),
            uri: None,
            lsp_version: RwSignal::new(1),
            lint_gen: Rc::new(std::cell::Cell::new(0)),
            editor: editors,
            win_origin: crate::per_pane::PerPane::new(id, self.panes(), Point::ZERO),
            caret: crate::per_pane::PerPane::new(id, self.panes(), None),
            pending_goto: RwSignal::new(None),
            disk_mtime: RwSignal::new(None),
            disk_changed: RwSignal::new(false),
            blame: Rc::new(RefCell::new(Vec::new())),
            inlay_hints: RwSignal::new(Vec::new()),
            ghost: RwSignal::new(None),
            large: false,
            encoding: RwSignal::new("UTF-8".to_string()),
            lint: Rc::new(RefCell::new(Vec::new())),
            analysis: Rc::new(RefCell::new(Vec::new())),
            migration_risks: Rc::new(RefCell::new(Vec::new())),
            annotations: RwSignal::new(Vec::new()),
            alerts: RwSignal::new(Vec::new()),
            evals: RwSignal::new(Vec::new()),
            caught: Rc::new(RefCell::new(Vec::new())),
            undo,
            undo_nav,
            tab_width: self.settings.get_untracked().tab_width,
            editorconfig: e_core::editorconfig::EditorConfig::default(),
        };
        self.editor.buffers.update(|bs| bs.push(buf));
        self.focused_active().set(Some(id));
    }

    /// Prompt for a path and save the active buffer there, then reopen it so it
    /// gets the right language, LSP, and git integration.
    pub fn save_active_as(&self) {
        if let Some(id) = self.focused_active_id() {
            self.save_buffer_as(id, true, |_| {});
        }
    }

    /// Prompt for a path and save buffer `id` there. The buffer is closed once
    /// written; with `reopen` the saved file opens in its place.
    pub(crate) fn save_buffer_as(
        &self,
        id: u64,
        reopen: bool,
        on_saved: impl FnOnce(bool) + 'static,
    ) {
        let state = *self;
        // The dialog's callback may fire more than once in principle; the
        // outcome is reported once.
        let on_saved = RefCell::new(Some(on_saved));
        let report = move |ok: bool| {
            if let Some(f) = on_saved.borrow_mut().take() {
                f(ok);
            }
        };
        let opts = floem::file::FileDialogOptions::new()
            .title("Save As")
            .force_starting_directory(self.root.get_untracked());
        floem::action::save_as(opts, move |info| {
            let Some(path) = info.and_then(|i| i.path.into_iter().next()) else {
                report(false);
                return;
            };
            // Read the text now, not when the sheet opened.
            let Some(buf) = state.buffer_by_id(id) else {
                report(false);
                return;
            };
            let content = buf.doc.text().to_string();
            match buffer::write_with_encoding(&path, &content, &buf.encoding.get_untracked()) {
                Ok(()) => {
                    state.force_close(id);
                    if reopen {
                        state.open_path(path);
                    }
                    report(true);
                }
                Err(e) => {
                    eprintln!("e: save as failed: {e:#}");
                    Self::notify(&format!("Could not save: {e:#}"));
                    report(false);
                }
            }
        });
    }

    pub fn buffer_by_id(&self, id: u64) -> Option<Buffer> {
        self.editor
            .buffers
            .with(|bs| bs.iter().find(|b| b.id == id).cloned())
    }

    /// The active-buffer signal of the focused pane.
    pub(crate) fn focused_active(&self) -> RwSignal<Option<u64>> {
        if self.editor.focused.get_untracked() == 1 {
            self.editor.active2
        } else {
            self.editor.active
        }
    }

    /// Buffer id active in the focused pane, tracked reactively.
    pub fn focused_active_id(&self) -> Option<u64> {
        if self.editor.focused.get() == 1 {
            self.editor.active2.get()
        } else {
            self.editor.active.get()
        }
    }

    /// Focus a buffer in the currently focused pane (e.g. clicking a tab).
    pub fn focus_buffer(&self, id: u64) {
        self.focused_active().set(Some(id));
    }

    pub fn is_pinned(&self, id: u64) -> bool {
        self.nav.pinned_tabs.with(|set| set.contains(&id))
    }

    pub fn toggle_pin(&self, id: u64) {
        self.nav.pinned_tabs.update(|set| {
            if !set.remove(&id) {
                set.insert(id);
            }
        });
    }

    /// Close every tab except `keep` (skipping pinned tabs).
    pub fn close_others(&self, keep: u64) {
        let ids: Vec<u64> = self.editor.buffers.with_untracked(|bs| {
            bs.iter()
                .map(|b| b.id)
                .filter(|id| *id != keep && !self.is_pinned(*id))
                .collect()
        });
        for id in ids {
            self.close(id);
        }
    }

    /// Move tab `src` to the position of `target` (drag-to-reorder).
    pub fn reorder_tab(&self, src: u64, target: u64) {
        if src == target {
            return;
        }
        self.editor.buffers.update(|bs| {
            let Some(from) = bs.iter().position(|b| b.id == src) else {
                return;
            };
            let b = bs.remove(from);
            let to = bs.iter().position(|x| x.id == target).unwrap_or(bs.len());
            bs.insert(to, b);
        });
    }

    pub(crate) fn buffer_id_by_path(&self, path: &str) -> Option<u64> {
        let canon = std::path::Path::new(path).canonicalize().ok();
        self.editor.buffers.with(|bs| {
            bs.iter()
                .find(|b| b.file.path.as_deref() == canon.as_deref())
                .map(|b| b.id)
        })
    }

    /// Restore the previous session for this workspace (open files, tabs, split).
    pub fn restore_session(&self) {
        let Some(data) = session::load(&self.root.get_untracked()) else {
            return;
        };
        // The tabs on screen open now; the rest after the window is up, one
        // per turn of the event loop. Each open reads, highlights and parses
        // its file, which with a dozen tabs held the first frame back.
        let mut visible = vec![data.active.clone()];
        if data.split {
            visible.push(data.active2.clone());
        }
        let (now, later): (Vec<String>, Vec<String>) = data
            .open
            .iter()
            .cloned()
            .partition(|p| visible.iter().any(|v| v.as_deref() == Some(p.as_str())));
        let now = if now.is_empty() {
            data.open.first().cloned().into_iter().collect()
        } else {
            now
        };
        let later: Vec<String> = later.into_iter().filter(|p| !now.contains(p)).collect();
        for p in &now {
            self.open_path(PathBuf::from(p));
        }
        for text in &data.untitled {
            self.new_untitled_with(text.clone());
        }
        self.restore_unsaved_edits(&data.modified);
        if let Some(a) = data
            .active
            .as_deref()
            .and_then(|a| self.buffer_id_by_path(a))
        {
            self.editor.active.set(Some(a));
        }
        if data.split {
            self.editor.split.set(true);
            if let Some(a2) = data
                .active2
                .as_deref()
                .and_then(|a| self.buffer_id_by_path(a))
            {
                self.editor.active2.set(Some(a2));
            }
        }
        if !later.is_empty() {
            // Until every tab is back, a saved session would be missing the
            // rest — and their unsaved edits.
            RESTORING.store(true, std::sync::atomic::Ordering::Relaxed);
            self.open_restored_later(later.into(), data.open, data.modified);
        }
    }

    /// Open the remaining restored tabs one at a time without taking focus,
    /// then put the tabs back in their saved order.
    pub(crate) fn open_restored_later(
        &self,
        mut rest: VecDeque<String>,
        order: Vec<String>,
        modified: Vec<session::ModifiedFile>,
    ) {
        let state = *self;
        let Some(next) = rest.pop_front() else {
            let rank = |b: &Buffer| {
                b.file
                    .path
                    .as_ref()
                    .and_then(|p| order.iter().position(|o| std::path::Path::new(o) == p))
                    .unwrap_or(usize::MAX)
            };
            self.editor.buffers.update(|bs| bs.sort_by_key(rank));
            RESTORING.store(false, std::sync::atomic::Ordering::Relaxed);
            self.save_session();
            return;
        };
        floem::action::exec_after(std::time::Duration::from_millis(15), move |_| {
            let focus = state.focused_active().get_untracked();
            state.open_path(PathBuf::from(&next));
            state.focused_active().set(focus);
            state.restore_unsaved_edits(&modified);
            state.open_restored_later(rest, order, modified);
        });
    }

    /// Put unsaved edits from the last run back over their files (hot exit).
    ///
    /// The restored text is an ordinary edit, so the tab is dirty and one undo
    /// returns to what is on disk. If the file changed on disk since — a `git
    /// pull` while `e` was closed — the conflict banner comes up instead of
    /// autosave quietly writing the old edits over it.
    pub(crate) fn restore_unsaved_edits(&self, modified: &[session::ModifiedFile]) {
        for m in modified {
            let Some(buf) = self
                .buffer_id_by_path(&m.path)
                .and_then(|id| self.buffer_by_id(id))
            else {
                continue;
            };
            let current = buf.doc.text().to_string();
            if current == m.text {
                continue;
            }
            let disk_now = std::fs::metadata(&m.path)
                .ok()
                .and_then(|md| md.modified().ok())
                .and_then(session::mtime_ms);
            buf.doc.edit_single(
                Selection::region(0, current.len()),
                &m.text,
                EditType::Other,
            );
            buf.dirty.set(true);
            if disk_now != m.disk_mtime_ms {
                buf.disk_changed.set(true);
            }
        }
    }

    /// Something that changes whenever unsaved text does: `None` when nothing
    /// is unsaved. The idle tick backs the session up only when this moves.
    pub fn unsaved_fingerprint(&self) -> Option<u64> {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let mut any = false;
        self.editor.buffers.with_untracked(|bs| {
            for b in bs {
                if b.file.path.is_none() || b.dirty.get_untracked() {
                    any = true;
                    (b.id, b.doc.rev(), b.dirty.get_untracked()).hash(&mut h);
                }
            }
        });
        any.then(|| h.finish())
    }

    /// Persist the current session.
    pub fn save_session(&self) {
        if RESTORING.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        session::save(&self.root.get_untracked(), &self.session_data());
    }

    /// Persist the current session off the UI thread.
    pub fn save_session_in_background(&self) {
        if RESTORING.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        session::save_in_background(self.root.get_untracked(), self.session_data());
    }

    pub(crate) fn session_data(&self) -> SessionData {
        let buffers = self.editor.buffers.get_untracked();
        let path_of = |id: Option<u64>| -> Option<String> {
            id.and_then(|i| buffers.iter().find(|b| b.id == i))
                .and_then(|b| b.file.path.as_ref())
                .map(|p| p.display().to_string())
        };
        let open: Vec<String> = buffers
            .iter()
            .filter_map(|b| b.file.path.as_ref().map(|p| p.display().to_string()))
            .collect();
        // Unsaved untitled scratch buffers with content, so a quit doesn't lose
        // them (they're recreated as untitled tabs on restore).
        let untitled: Vec<String> = buffers
            .iter()
            .filter(|b| b.file.path.is_none())
            .map(|b| b.doc.text().to_string())
            .filter(|t| !t.trim().is_empty())
            .collect();
        // Unsaved edits to real files, with the mtime the buffer last agreed
        // with — unknown (`None`) once a conflict is already pending, so the
        // restore raises it again.
        let modified: Vec<session::ModifiedFile> = buffers
            .iter()
            .filter(|b| b.dirty.get_untracked())
            .filter_map(|b| {
                let path = b.file.path.as_ref()?;
                Some(session::ModifiedFile {
                    path: path.display().to_string(),
                    text: b.doc.text().to_string(),
                    disk_mtime_ms: if b.disk_changed.get_untracked() {
                        None
                    } else {
                        b.disk_mtime.get_untracked().and_then(session::mtime_ms)
                    },
                })
            })
            .collect();
        SessionData {
            open,
            untitled,
            modified,
            active: path_of(self.editor.active.get_untracked()),
            active2: path_of(self.editor.active2.get_untracked()),
            split: self.editor.split.get_untracked(),
        }
    }

    /// Toggle the two-pane split view.
    /// The editor panes, for a buffer's per-pane state.
    pub(crate) fn panes(&self) -> crate::per_pane::Panes {
        crate::per_pane::Panes {
            split: self.editor.split,
            focused: self.editor.focused,
            active: [self.editor.active, self.editor.active2],
        }
    }

    /// Move the keyboard to the other editor pane.
    pub fn focus_other_pane(&self) {
        if !self.editor.split.get_untracked() {
            return;
        }
        let other = 1 - self.editor.focused.get_untracked().min(1);
        self.editor.focused.set(other);
        let view = self
            .active_buffer()
            .and_then(|b| b.editor.get_in(other))
            .and_then(|ed| ed.editor_view_id.get_untracked());
        if let Some(view) = view {
            view.request_focus();
        }
    }

    pub fn toggle_split(&self) {
        let on = !self.editor.split.get_untracked();
        self.editor.split.set(on);
        if on {
            if self.editor.active2.get_untracked().is_none() {
                self.editor.active2.set(self.editor.active.get_untracked());
            }
            self.editor.focused.set(1);
        } else {
            self.editor.focused.set(0);
            // The right pane's editors are gone with its view.
            for b in self.editor.buffers.get_untracked() {
                b.editor.set_in(1, None);
            }
        }
    }

    /// Open a file by path. If it's already open, just focus it.
    pub fn open_path(&self, path: PathBuf) {
        // A buffer lives as long as the app, whoever opens it. Created inside
        // an effect — the agent socket's, the Finder's — its signals would
        // belong to that effect and be disposed the next time it ran, and
        // reading one then panics.
        let app = *self;
        floem::reactive::with_scope(self.cx, move || app.open_path_in_app_scope(path))
    }

    pub(crate) fn open_path_in_app_scope(&self, path: PathBuf) {
        let canon = path.canonicalize().unwrap_or(path);

        // Already open? Focus the existing tab.
        let existing = self.editor.buffers.with(|bs| {
            bs.iter()
                .find(|b| b.file.path.as_deref() == Some(canon.as_path()))
                .map(|b| b.id)
        });
        if let Some(id) = existing {
            self.focused_active().set(Some(id));
            return;
        }

        // A binary file opened as text is one keystroke and an autosave from
        // being corrupted (`database.sqlite`), and a huge log freezes the window.
        if let Err(why) = buffer::check_openable(&canon) {
            let name = canon.file_name().and_then(|n| n.to_str()).unwrap_or("file");
            eprintln!("e: not opening {}: {why}", canon.display());
            Self::notify(&format!("Not opening {name}: {why}"));
            return;
        }
        let (content, encoding) = match buffer::read_with_encoding(&canon) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("e: open failed: {e:#}");
                Self::notify(&format!("Could not open: {e:#}"));
                return;
            }
        };

        let id = self.next_id.get();
        self.next_id.set(id + 1);

        let file = FileInfo::for_path(canon.clone());
        let language = file.language;
        let uri = file.path.as_ref().map(|p| path_to_uri(p));

        // Very large files skip tree-sitter highlighting (and other per-edit
        // work) to stay responsive.
        let large = content.len() > 1_000_000;
        // Kept between edits for incremental highlighting, where supported.
        let (syntax, initial_spans) = if large {
            (None, Vec::new())
        } else if content.len() <= INCREMENTAL_HIGHLIGHT_LIMIT {
            match e_core::syntax::SyntaxTree::new(language, &content) {
                Some((tree, spans)) => (Some(tree), spans),
                None => (None, highlight_lines(language, &content)),
            }
        } else {
            (None, highlight_lines(language, &content))
        };
        let syntax = Rc::new(RefCell::new(syntax));
        let highlights: Highlights = Rc::new(RefCell::new(initial_spans));

        let doc = Rc::new(TextDocument::new(self.cx, content.clone()));
        // Keep/auto-indent on newline (matches editor expectations).
        doc.auto_indent.set(true);

        // Git change markers vs HEAD. Reading HEAD's copy is a `git`
        // process, so it happens in the background; the markers appear when
        // it lands (and every edit after recomputes them against it).
        let head_text: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let git_marks: GitMarks = Rc::new(RefCell::new(Vec::new()));
        // A test file shows how its tests last ended, from the runs so far.
        let test_marks: TestMarks = Rc::new(RefCell::new(match &file.path {
            Some(path) => self.test_marks_for(path, &content),
            None => Default::default(),
        }));
        if let Some(path) = file.path.clone() {
            let (head, marks, doc) = (head_text.clone(), git_marks.clone(), doc.clone());
            self.spawn_bg(
                move || git::head_text(&path),
                move |text: Option<String>| {
                    if let Some(h) = &text {
                        let now = doc.text().to_string();
                        let lc = now.split_inclusive('\n').count().max(1);
                        *marks.borrow_mut() = git::marks(h, &now, lc);
                        doc.cache_rev().update(|r| *r += 1);
                    }
                    *head.borrow_mut() = text;
                },
            );
        }
        let dirty = RwSignal::new(false);
        let saved = Rc::new(RefCell::new(doc.text()));
        let version = RwSignal::new(1i64);

        // Branching undo tree, restored from disk when it still matches.
        let undo_path = file.path.as_ref().map(|p| undo_store_path(p));
        let undo = {
            let loaded = undo_path
                .as_ref()
                .filter(|_| !large)
                .and_then(|p| e_core::undotree::UndoTree::load(p));
            let t = match loaded {
                // Restore only if the tree still matches the file on disk.
                Some(mut t) if !t.is_empty() => {
                    if t.sync_to(&content) {
                        t
                    } else {
                        e_core::undotree::UndoTree::new(content.clone())
                    }
                }
                // A very large file keeps no history tree: it would hold a
                // second full copy and diff the whole file on every edit.
                // Floem's own undo still works.
                _ if large => e_core::undotree::UndoTree::new(String::new()),
                _ => e_core::undotree::UndoTree::new(content.clone()),
            };
            Rc::new(RefCell::new(t))
        };
        let undo_nav = Rc::new(std::cell::Cell::new(false));

        // Hand the document to the language server, if we have one.
        if let (Some(lang_id), Some(uri)) = (self.lsp_language_id(language), uri.as_ref()) {
            // Starts every server for this language; each needs its own didOpen.
            self.ensure_lsp(language);
            // One copy of the text for every server, not one each.
            let shared = std::sync::Arc::new(content.clone());
            for client in self.lsp_clients_for(language) {
                client.did_open_shared(uri, lang_id, 1, shared.clone());
            }
        }

        // On every edit: keep both panes' carets on the text, mark dirty,
        // re-highlight, invalidate the layout cache, and notify the language
        // server.
        let editors = crate::per_pane::PerPane::new(id, self.panes(), None);
        {
            let doc = doc.clone();
            let saved = saved.clone();
            let highlights = highlights.clone();
            let git_marks = git_marks.clone();
            let test_marks = test_marks.clone();
            let test_path = file.path.clone();
            let head_text = head_text.clone();
            let app = *self;
            let uri = uri.clone();
            let undo = undo.clone();
            let undo_nav = undo_nav.clone();
            let undo_path = undo_path.clone();
            // Generation of the newest scheduled background highlight. Bumped on
            // every edit so a job that finishes after a later keystroke is dropped
            // instead of painting stale colours.
            let hl_gen: Rc<std::cell::Cell<u64>> = Rc::new(std::cell::Cell::new(0));
            let syntax = syntax.clone();
            doc.clone().add_on_update(move |update| {
                crate::state::settle_carets(editors, &update, &doc.text());
                app.editor.last_edit.set(now_ms());
                // A peek shows the hunk as it was; an edit may have moved it.
                app.close_hunk_peek();
                // One copy of the text per edit, shared with the language
                // servers rather than copied into each.
                let shared = std::sync::Arc::new(doc.text().to_string());
                let text: &str = &shared;
                // Undoing back to what's on disk leaves nothing to save. Set
                // only on a change: every subscriber (title, tab, status)
                // re-renders on a set, even to the same value.
                let changed = !same_text(&saved.borrow(), text);
                if dirty.get_untracked() != changed {
                    dirty.set(changed);
                }
                if let Some(path) = &test_path {
                    // A test's mark moves with its declaration.
                    if app.tests.outcomes.with_untracked(|o| !o.is_empty()) {
                        *test_marks.borrow_mut() = app.test_marks_for(path, text);
                    }
                    // And a breakpoint, or a logged exception, with its line.
                    if let Some(old) = update.old_text() {
                        app.follow_breakpoints(path, old, text);
                        app.follow_caught(path, old, text);
                    }
                }
                if !undo_nav.get() && !large {
                    let now = now_ms() as u64;
                    // Do the recording (and any save) inside a scope so the
                    // mutable borrow is dropped before undo_rev.update runs the
                    // undo-tree view's effect, which borrows `undo` again.
                    let (recorded, json) = {
                        let mut t = undo.borrow_mut();
                        let rec = t.record(text, now, 700);
                        let json = if rec && undo_path.is_some() {
                            t.json_if_due(now)
                        } else {
                            None
                        };
                        (rec, json)
                    };
                    // Written off the UI thread: the history is saved while
                    // the user types.
                    if let (Some(json), Some(p)) = (json, undo_path.clone()) {
                        std::thread::spawn(move || {
                            let _ = buffer::write(&p, &json);
                        });
                    }
                    if recorded {
                        app.editor.undo_rev.update(|r| *r += 1);
                    }
                }
                let incremental =
                    text.len() <= INCREMENTAL_HIGHLIGHT_LIMIT && syntax.borrow().is_some();
                if !incremental {
                    // Grown past the limit: the tree would fall behind, so
                    // it goes for good and the paths below take over.
                    syntax.borrow_mut().take();
                }
                if large {
                    // No highlighting for very large files.
                } else if incremental {
                    // Supersede any background highlight still running from
                    // when this file was larger.
                    hl_gen.set(hl_gen.get() + 1);
                    if let Some(tree) = syntax.borrow_mut().as_mut() {
                        tree.update(text, &mut highlights.borrow_mut());
                    }
                    if let Some(head) = &*head_text.borrow() {
                        let lc = text.split_inclusive('\n').count().max(1);
                        *git_marks.borrow_mut() = git::marks(head, text, lc);
                    }
                } else if text.len() <= SYNC_HIGHLIGHT_LIMIT {
                    *highlights.borrow_mut() = highlight_lines(language, text);
                    if let Some(head) = &*head_text.borrow() {
                        let lc = text.split_inclusive('\n').count().max(1);
                        *git_marks.borrow_mut() = git::marks(head, text, lc);
                    }
                } else {
                    // Too expensive to keep inline — see SYNC_HIGHLIGHT_LIMIT.
                    // The previous colours stay on screen until the new ones
                    // land, which reads as a brief lag rather than a freeze.
                    let gen = hl_gen.get() + 1;
                    hl_gen.set(gen);
                    schedule_highlight(
                        app.cx,
                        gen,
                        hl_gen.clone(),
                        language,
                        text.to_string(),
                        head_text.borrow().clone(),
                        highlights.clone(),
                        git_marks.clone(),
                        doc.clone(),
                    );
                }
                doc.cache_rev().update(|r| *r += 1);

                if let Some(uri) = uri.as_ref() {
                    let clients = app.lsp_clients_for(language);
                    if !clients.is_empty() {
                        let v = version.get() + 1;
                        version.set(v);
                        for client in clients {
                            client.did_change_shared(uri, v, shared.clone());
                        }
                    }
                }
                // Trigger completion (LSP + snippets + Laravel helpers).
                app.autocomplete_after_edit(id);
                // Request an inline AI suggestion (debounced; no-op if disabled).
                app.request_ghost(id);
                // Laravel query-builder lint (unknown columns).
                app.refresh_lint(id);
                app.follow_evals(id);
            });
        }

        let disk_mtime = file
            .path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok());

        // Per-file EditorConfig (indent / trailing whitespace / final newline).
        let ec = e_core::editorconfig::resolve(&canon);
        let ec_tab = ec
            .effective_tab_width()
            .unwrap_or_else(|| self.settings.get_untracked().tab_width)
            .clamp(1, 16);
        let buf = Buffer {
            id,
            file,
            doc,
            dirty,
            saved,
            highlights,
            diag_lines: Rc::new(RefCell::new(Vec::new())),
            git_marks,
            head_text,
            bp_marks: Rc::new(RefCell::new(Default::default())),
            stop_line: Rc::new(RefCell::new(None)),
            test_marks: test_marks.clone(),
            find_marks: Rc::new(RefCell::new(Vec::new())),
            bracket_marks: Rc::new(RefCell::new(Vec::new())),
            uri,
            lsp_version: version,
            lint_gen: Rc::new(std::cell::Cell::new(0)),
            editor: editors,
            win_origin: crate::per_pane::PerPane::new(id, self.panes(), Point::ZERO),
            caret: crate::per_pane::PerPane::new(id, self.panes(), None),
            pending_goto: RwSignal::new(None),
            disk_mtime: RwSignal::new(disk_mtime),
            disk_changed: RwSignal::new(false),
            blame: Rc::new(RefCell::new(Vec::new())),
            inlay_hints: RwSignal::new(Vec::new()),
            ghost: RwSignal::new(None),
            large,
            encoding: RwSignal::new(encoding),
            lint: Rc::new(RefCell::new(Vec::new())),
            analysis: Rc::new(RefCell::new(Vec::new())),
            migration_risks: Rc::new(RefCell::new(Vec::new())),
            annotations: RwSignal::new(Vec::new()),
            alerts: RwSignal::new(Vec::new()),
            evals: RwSignal::new(Vec::new()),
            caught: Rc::new(RefCell::new(Vec::new())),
            undo,
            undo_nav,
            tab_width: ec_tab,
            editorconfig: ec,
        };
        self.editor.buffers.update(|bs| bs.push(buf));
        self.focused_active().set(Some(id));
        self.sync_bp_marks(&canon.to_string_lossy());
        self.sync_caught_marks();
        self.load_blame(id);
        self.refresh_lint(id);
        self.request_inlay_hints(id);
        self.check_migration(id);
    }

    /// Close a tab; focus a neighbour if it was active.
    /// Close a tab, prompting first if it has unsaved changes.
    pub fn close(&self, id: u64) {
        let dirty = self
            .editor
            .buffers
            .with_untracked(|bs| {
                bs.iter()
                    .find(|b| b.id == id)
                    .map(|b| b.dirty.get_untracked())
            })
            .unwrap_or(false);
        if dirty {
            self.editor.close_confirm.set(Some(id));
        } else {
            self.force_close(id);
        }
    }

    /// Save the pending buffer, then close it — once the write has landed.
    /// Closing straight away lost the edits whenever saving finished later
    /// (format on save, the Save As sheet) or failed.
    pub fn confirm_close_save(&self) {
        if let Some(id) = self.editor.close_confirm.get_untracked() {
            self.editor.close_confirm.set(None);
            let app = *self;
            self.save_buffer(id, false, move |saved| {
                if saved {
                    app.force_close(id);
                }
            });
        }
    }

    /// Discard changes and close the pending buffer.
    pub fn confirm_close_discard(&self) {
        if let Some(id) = self.editor.close_confirm.get_untracked() {
            self.editor.close_confirm.set(None);
            self.force_close(id);
        }
    }

    pub fn cancel_close(&self) {
        self.editor.close_confirm.set(None);
    }

    pub fn force_close(&self, id: u64) {
        let mut focus_next = None;
        let mut closed_uri = None;
        let mut closed_lang = None;
        self.editor.buffers.update(|bs| {
            if let Some(pos) = bs.iter().position(|b| b.id == id) {
                closed_uri = bs[pos].uri.clone();
                closed_lang = Some(bs[pos].file.language);
                bs.remove(pos);
                if !bs.is_empty() {
                    let n = pos.min(bs.len() - 1);
                    focus_next = Some(bs[n].id);
                }
            }
        });
        if self.editor.active.get_untracked() == Some(id) {
            self.editor.active.set(focus_next);
        }
        if self.editor.active2.get_untracked() == Some(id) {
            self.editor.active2.set(focus_next);
        }
        if let (Some(uri), Some(lang)) = (closed_uri, closed_lang) {
            for client in self.lsp_clients_for(lang) {
                client.did_close(&uri);
            }
        }
    }

    pub fn active_buffer(&self) -> Option<Buffer> {
        let active = self.focused_active_id()?;
        self.editor
            .buffers
            .with(|bs| bs.iter().find(|b| b.id == active).cloned())
    }

    /// Strip trailing whitespace and ensure a final newline in a buffer.
    pub(crate) fn trim_buffer(buf: &Buffer) {
        let text = buf.doc.text().to_string();
        let (edits, needs_newline) = trailing_trim_edits(&text);
        if edits.is_empty() && !needs_newline {
            return;
        }
        let mut batch: Vec<(usize, usize, String)> = edits
            .into_iter()
            .map(|(s, e)| (s, e, String::new()))
            .collect();
        if needs_newline {
            batch.push((text.len(), text.len(), "\n".to_string()));
        }
        apply_batch(&buf.doc, batch, EditType::Delete);
    }

    /// Save the active buffer to disk (formatting / trimming first, if enabled).
    pub fn save_active(&self) {
        if let Some(id) = self.focused_active_id() {
            self.save_buffer(id, true, |_| {});
        }
    }

    /// Save buffer `id`: format it if the settings say so, trim it, write it,
    /// and tell `on_saved` whether it reached the disk. Everything goes by id,
    /// never "the active buffer": formatting finishes later, and by then the
    /// user may be in another tab — or have closed this one. An untitled
    /// buffer goes through Save As; `reopen` decides whether the saved file
    /// then opens in its place.
    pub fn save_buffer(&self, id: u64, reopen: bool, on_saved: impl FnOnce(bool) + 'static) {
        let Some(buf) = self.buffer_by_id(id) else {
            on_saved(false);
            return;
        };
        if buf.file.path.is_none() {
            self.save_buffer_as(id, reopen, on_saved);
            return;
        }
        if self.settings.get_untracked().format_on_save {
            let app = *self;
            self.format_buffer_then(buf, move || on_saved(app.write_buffer(id)));
            return;
        }
        on_saved(self.write_buffer(id));
    }

    /// The saving half of [`Self::save_buffer`]: trim, fix the final newline,
    /// write, and tell everyone who cares. Returns whether the file was written.
    pub(crate) fn write_buffer(&self, id: u64) -> bool {
        // Closed while a format was running: the close decided its fate.
        let Some(buf) = self.buffer_by_id(id) else {
            return false;
        };
        let Some(path) = buf.file.path.clone() else {
            return false;
        };
        // EditorConfig `trim_trailing_whitespace` overrides the global setting.
        let ec = buf.editorconfig.clone();
        let trim = ec
            .trim_trailing_whitespace
            .unwrap_or_else(|| self.settings.get_untracked().trim_on_save);
        if trim {
            Self::trim_buffer(&buf);
        }
        // EditorConfig `insert_final_newline`: ensure exactly one trailing \n.
        if ec.insert_final_newline == Some(true) {
            let t = buf.doc.text().to_string();
            if !t.is_empty() && !t.ends_with('\n') {
                let len = t.len();
                buf.doc.edit_single(
                    Selection::caret(len),
                    "\n",
                    floem::views::editor::core::editor::EditType::InsertChars,
                );
            }
        }
        if Self::disk_changed_unnoticed(&buf) {
            Self::notify(&format!(
                "{} changed on disk. Reload it, or save again to keep your version.",
                buf.file.display_name()
            ));
            return false;
        }
        let text = buf.doc.text().to_string();
        match buffer::write_with_encoding(&path, &text, &buf.encoding.get_untracked()) {
            Ok(()) => {
                buf.mark_saved();
                buf.disk_changed.set(false);
                Self::refresh_disk_mtime(&buf);
                self.fs_rev.update(|r| *r += 1);
                self.load_blame(buf.id);
                // PHPStan reads the file, so it can only run once it is written.
                self.run_phpstan(buf.id);
                self.check_migration(buf.id);
                self.request_inlay_hints(buf.id);
                eprintln!("e: saved {}", path.display());
                self.laravel_touch(&path);
                self.rules_touch(&path);
                self.tests_after_save(&path, buf.file.language);
                self.clear_caught_in(&path);
                if let Some(uri) = buf.uri.as_ref() {
                    for client in self.lsp_clients_for(buf.file.language) {
                        client.did_save(uri, &text);
                    }
                }
                self.request_outline();
                true
            }
            Err(e) => {
                eprintln!("e: save failed: {e:#}");
                Self::notify(&format!(
                    "Could not save {}: {e:#}",
                    buf.file.display_name()
                ));
                false
            }
        }
    }

    /// Whether the file changed on disk since the buffer last agreed with it,
    /// without anyone having been told. If so it raises the conflict banner and
    /// returns true, and the write must not happen: the idle poll runs every
    /// 500 ms, and a save inside that window would otherwise overwrite a `git
    /// checkout` or a formatter's work unseen. Once the banner is up, an
    /// explicit save is the user choosing their version, so it goes ahead.
    pub(crate) fn disk_changed_unnoticed(buf: &Buffer) -> bool {
        if buf.disk_changed.get_untracked() {
            return false;
        }
        let Some(known) = buf.disk_mtime.get_untracked() else {
            return false;
        };
        let now = buf
            .file
            .path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok());
        if now.is_some_and(|t| t != known) {
            buf.disk_changed.set(true);
            return true;
        }
        false
    }
}
