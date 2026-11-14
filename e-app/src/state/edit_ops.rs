//! Editing the text: multiple cursors, find and replace, brackets, inlay hints, the outline, merge conflicts.

use super::*;

impl AppState {
    // ---- Local rename --------------------------------------------------

    /// Open the rename bar for the identifier under the cursor.
    pub fn open_rename(&self) {
        self.rename.laravel.set(None);
        if self.open_laravel_rename() {
            return;
        }
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(editor) = buf.editor.get_untracked() else {
            return;
        };
        let offset = editor.cursor.get_untracked().offset();
        let text = buf.doc.text().to_string();
        let word = word_at(&text, offset);
        if word.is_empty() {
            return;
        }
        let r = self.rename;
        r.word.set(word.clone());
        r.new_name.set(word);
        r.open.set(true);
    }

    pub fn close_rename(&self) {
        self.rename.open.set(false);
    }

    /// Multi-cursor (⌘D): expand the caret to its word, or add a cursor at the
    /// next occurrence of the current selection.
    pub fn select_next_occurrence(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(editor) = buf.editor.get_untracked() else {
            return;
        };
        let cursor = editor.cursor.get_untracked();
        let CursorMode::Insert(sel) = cursor.mode.clone() else {
            return;
        };
        let text = buf.doc.text().to_string();
        let regions = sel.regions().to_vec();
        let all_carets = regions.iter().all(|r| r.start == r.end);

        let new_sel = if all_carets {
            // Expand each caret to the surrounding word.
            let mut s = Selection::new();
            for r in &regions {
                let (a, b) = word_range(&text, r.max());
                if b > a {
                    s.add_region(SelRegion::new(a, b, None));
                } else {
                    s.add_region(*r);
                }
            }
            s
        } else {
            // Add the next occurrence of the last non-empty region's text.
            let mut s = sel.clone();
            if let Some(last) = regions.iter().rev().find(|r| r.max() > r.min()) {
                let word = text[last.min()..last.max()].to_string();
                if let Some(pos) = find_next(&text, &word, last.max()) {
                    s.add_region(SelRegion::new(pos, pos + word.len(), None));
                }
            }
            s
        };

        editor
            .cursor
            .set(Cursor::new(CursorMode::Insert(new_sel), None, None));
    }

    /// Place a cursor on every occurrence of the current word/selection (⌘⇧L).
    pub fn select_all_occurrences(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(editor) = buf.editor.get_untracked() else {
            return;
        };
        let cursor = editor.cursor.get_untracked();
        let CursorMode::Insert(sel) = cursor.mode.clone() else {
            return;
        };
        let text = buf.doc.text().to_string();
        let regions = sel.regions().to_vec();
        let all_carets = regions.iter().all(|r| r.start == r.end);

        let (term, whole) = if all_carets {
            let Some(r) = regions.last() else {
                return;
            };
            let (a, b) = word_range(&text, r.max());
            if b <= a {
                return;
            }
            (text[a..b].to_string(), true)
        } else {
            let Some(last) = regions.iter().rev().find(|r| r.max() > r.min()) else {
                return;
            };
            (text[last.min()..last.max()].to_string(), false)
        };

        if term.is_empty() {
            return;
        }
        let occ = find_all_opts(&text, &term, true, whole, false);
        if occ.is_empty() {
            return;
        }
        let mut s = Selection::new();
        for (a, b) in occ {
            s.add_region(SelRegion::new(a, b, None));
        }
        editor
            .cursor
            .set(Cursor::new(CursorMode::Insert(s), None, None));
    }

    /// Add a caret one line above the top-most caret / below the bottom-most
    /// caret at the same column (column editing). `delta` is -1 (above) or +1.
    pub(crate) fn add_cursor_line(&self, delta: i64) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(editor) = buf.editor.get_untracked() else {
            return;
        };
        let cursor = editor.cursor.get_untracked();
        let CursorMode::Insert(sel) = cursor.mode.clone() else {
            return;
        };
        let regions = sel.regions().to_vec();
        if regions.is_empty() {
            return;
        }
        // Anchor on the extreme caret in the direction we're growing.
        let anchor = if delta < 0 {
            regions.iter().map(|r| r.max()).min()
        } else {
            regions.iter().map(|r| r.max()).max()
        };
        let Some(anchor) = anchor else {
            return;
        };
        let (line, col) = editor.offset_to_line_col(anchor);
        let target = line as i64 + delta;
        if target < 0 {
            return;
        }
        let line_count = {
            let text = buf.doc.text();
            text.line_of_offset(text.len()) + 1
        };
        if target as usize >= line_count {
            return;
        }
        let new_offset = editor.offset_of_line_col(target as usize, col);
        let mut s = sel.clone();
        s.add_region(SelRegion::new(new_offset, new_offset, None));
        editor
            .cursor
            .set(Cursor::new(CursorMode::Insert(s), None, None));
    }

    /// Add a caret on the line above (column editing).
    pub fn add_cursor_above(&self) {
        self.add_cursor_line(-1);
    }

    /// Add a caret on the line below (column editing).
    pub fn add_cursor_below(&self) {
        self.add_cursor_line(1);
    }

    // ---- Merge conflicts ------------------------------------------------

    /// Expand an Emmet abbreviation before the cursor (HTML-family languages).
    /// Returns true when something was expanded (so Tab is consumed).
    pub fn try_emmet_expand(&self) -> bool {
        let Some(buf) = self.active_buffer() else {
            return false;
        };
        if !matches!(
            buf.file.language,
            Language::Html | Language::Php | Language::Blade | Language::Vue | Language::Svelte
        ) {
            return false;
        }
        let Some(editor) = buf.editor.get_untracked() else {
            return false;
        };
        let end = editor.cursor.get_untracked().offset();
        let text = buf.doc.text().to_string();
        let end = end.min(text.len());
        let line_start = text[..end].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let line_before = &text[line_start..end];

        let Some((rel_start, abbr)) = crate::emmet::abbreviation_at(line_before) else {
            return false;
        };
        if !crate::emmet::is_expandable(&abbr) {
            return false;
        }
        let unit = " ".repeat(self.settings.get_untracked().tab_width.clamp(1, 8));
        let Some(markup) = crate::emmet::expand(&abbr, &unit) else {
            return false;
        };

        // Re-indent continuation lines to the current line's indentation.
        let base = line_indent(&text, line_start);
        let markup = markup.replace('\n', &format!("\n{base}"));
        let caret = markup.find('\0').unwrap_or(markup.len());
        let markup = markup.replace('\0', "");

        let start = line_start + rel_start;
        buf.doc.edit_single(
            Selection::region(start, end),
            &markup,
            EditType::InsertChars,
        );
        let pos = start + caret;
        editor.cursor.set(Cursor::new(
            CursorMode::Insert(Selection::caret(pos)),
            None,
            None,
        ));
        true
    }

    /// Convert the active buffer's line endings to CRLF (`true`) or LF.
    pub fn set_line_ending(&self, crlf: bool) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let text = buf.doc.text().to_string();
        let lf = text.replace("\r\n", "\n");
        let new = if crlf { lf.replace('\n', "\r\n") } else { lf };
        if new == text {
            return;
        }
        let mut it = std::iter::once((Selection::region(0, text.len()), new.as_str()));
        buf.doc.edit(&mut it, EditType::InsertChars);
    }

    /// Whether the active buffer contains conflict markers.
    ///
    /// Asked on every caret move (the conflict bar follows the caret), so the
    /// answer is kept per text revision instead of copying the file each time.
    pub fn active_has_conflicts(&self) -> bool {
        thread_local! {
            static LAST: std::cell::Cell<Option<(u64, u64, bool)>> = const { std::cell::Cell::new(None) };
        }
        let Some(b) = self.active_buffer() else {
            return false;
        };
        let rev = b.doc.rev();
        if let Some((id, r, has)) = LAST.get() {
            if id == b.id && r == rev {
                return has;
            }
        }
        let has = b.doc.text().to_string().contains("<<<<<<<");
        LAST.set(Some((b.id, rev, has)));
        has
    }

    /// The conflict block containing the caret: `(start, end, current, incoming)`.
    pub(crate) fn active_conflict_block(&self) -> Option<(usize, usize, String, String)> {
        let buf = self.active_buffer()?;
        let editor = buf.editor.get_untracked()?;
        let offset = editor.cursor.get_untracked().offset();
        let text = buf.doc.text().to_string();
        find_conflict(&text, offset)
    }

    /// Resolve the conflict at the caret: 0 = current, 1 = incoming, 2 = both.
    pub fn resolve_conflict(&self, choice: u8) {
        let Some((start, end, current, incoming)) = self.active_conflict_block() else {
            return;
        };
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let replacement = match choice {
            0 => current,
            1 => incoming,
            _ => format!("{current}{incoming}"),
        };
        let mut it = std::iter::once((Selection::region(start, end), replacement.as_str()));
        buf.doc.edit(&mut it, EditType::InsertChars);
    }

    // ---- Find in file --------------------------------------------------

    pub fn open_find(&self) {
        self.find.open.set(true);
        self.find.replace_open.set(false);
    }

    /// Open the find bar with the replace row expanded.
    pub fn open_replace(&self) {
        self.find.open.set(true);
        self.find.replace_open.set(true);
    }

    pub fn close_find(&self) {
        self.find.open.set(false);
        self.find.matches.set(Vec::new());
        if let Some(buf) = self.active_buffer() {
            *buf.find_marks.borrow_mut() = Vec::new();
            buf.doc.cache_rev().update(|r| *r += 1);
        }
    }

    /// Recompute matches for the current query (called as the query changes).
    pub fn run_find(&self) {
        let query = self.find.query.get_untracked();
        let Some(buf) = self.active_buffer() else {
            return;
        };
        if query.is_empty() {
            self.find.matches.set(Vec::new());
            *buf.find_marks.borrow_mut() = Vec::new();
            buf.doc.cache_rev().update(|r| *r += 1);
            return;
        }
        let text = buf.doc.text().to_string();
        let matches = find_all_opts(
            &text,
            &query,
            self.find.case_sensitive.get_untracked(),
            self.find.whole_word.get_untracked(),
            self.find.use_regex.get_untracked(),
        );
        self.find.matches.set(matches);
        self.find.current.set(0);
        self.apply_find_marks();
    }

    /// Replace the current match with the replacement text, then re-search.
    pub fn replace_current(&self) {
        let matches = self.find.matches.get_untracked();
        if matches.is_empty() {
            return;
        }
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let cur = self.find.current.get_untracked().min(matches.len() - 1);
        let (s, e) = matches[cur];
        let rep = self.find.replace.get_untracked();
        let sel = Selection::region(s, e);
        let mut it = std::iter::once((sel, rep.as_str()));
        buf.doc.edit(&mut it, EditType::InsertChars);
        self.run_find();
    }

    /// Replace every match with the replacement text in one edit.
    pub fn replace_all(&self) {
        let matches = self.find.matches.get_untracked();
        if matches.is_empty() {
            return;
        }
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let rep = self.find.replace.get_untracked();
        let edits: Vec<(Selection, String)> = matches
            .iter()
            .map(|(s, e)| (Selection::region(*s, *e), rep.clone()))
            .collect();
        let mut it = edits.iter().map(|(s, t)| (s.clone(), t.as_str()));
        buf.doc.edit(&mut it, EditType::InsertChars);
        self.run_find();
    }

    /// Rebuild per-line highlight spans and move the caret to the current match.
    pub(crate) fn apply_find_marks(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let matches = self.find.matches.get_untracked();
        let cur = self.find.current.get_untracked();
        let text = buf.doc.text().to_string();
        let starts = line_starts(&text);
        let mut lines: Vec<Vec<FindSpan>> = vec![Vec::new(); starts.len()];
        for (idx, (s, e)) in matches.iter().enumerate() {
            let line = line_of(&starts, *s);
            let ls = starts[line];
            lines[line].push(FindSpan {
                start: s - ls,
                end: e - ls,
                current: idx == cur,
            });
        }
        *buf.find_marks.borrow_mut() = lines;
        buf.doc.cache_rev().update(|r| *r += 1);

        if let Some(editor) = buf.editor.get_untracked() {
            if let Some((s, _)) = matches.get(cur) {
                editor.cursor.set(Cursor::new(
                    CursorMode::Insert(Selection::caret(*s)),
                    None,
                    None,
                ));
            }
        }
    }

    pub fn find_next(&self) {
        let n = self.find.matches.with(|m| m.len());
        if n == 0 {
            return;
        }
        self.find
            .current
            .set((self.find.current.get_untracked() + 1) % n);
        self.apply_find_marks();
    }

    pub fn find_prev(&self) {
        let n = self.find.matches.with(|m| m.len());
        if n == 0 {
            return;
        }
        let cur = self.find.current.get_untracked();
        self.find.current.set((cur + n - 1) % n);
        self.apply_find_marks();
    }

    /// Recompute the matching-bracket highlight for the active buffer and
    /// repaint. Called from a cursor-tracking effect.
    pub fn update_bracket_marks(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        if buf.large {
            return;
        }
        let Some(editor) = buf.editor.get_untracked() else {
            return;
        };
        let offset = editor.cursor.get_untracked().offset();
        // Bumping `cache_rev` throws away every line's layout, so it happens
        // only when the marks change — not on each caret move — and the file
        // is copied only when the caret is next to a bracket at all.
        let rope = buf.doc.text();
        // Byte reads: a caret after "å" sits mid-character for a slice.
        let near = [offset.checked_sub(1), Some(offset)]
            .into_iter()
            .flatten()
            .filter(|&o| o < rope.len())
            .any(|o| b"()[]{}".contains(&rope.byte_at(o)));
        let marks = if near {
            compute_bracket_marks(&rope.to_string(), offset)
        } else {
            Vec::new()
        };
        if *buf.bracket_marks.borrow() != marks {
            *buf.bracket_marks.borrow_mut() = marks;
            buf.doc.cache_rev().update(|r| *r += 1);
        }
    }

    /// Load the document outline for the active buffer (LSP documentSymbol).
    /// Request LSP inlay hints for a buffer and store them as phantom text.
    pub fn request_inlay_hints(&self, id: u64) {
        if !self.settings.get_untracked().inlay_hints {
            return;
        }
        let Some(buf) = self.buffer_by_id(id) else {
            return;
        };
        if buf.large {
            return;
        }
        if self.lsp_language_id(buf.file.language).is_none() {
            return;
        }
        let (Some(client), Some(uri)) = (self.lsp_for_language(buf.file.language), buf.uri.clone())
        else {
            return;
        };
        let end_line = buf.doc.text().to_string().split('\n').count().max(1) as u32;
        let hints_sig = buf.inlay_hints;
        let cache = buf.doc.cache_rev();
        let send = create_ext_action(self.cx, move |hints: Vec<(u32, u32, String)>| {
            // Only repaint when the hints actually changed.
            if hints != hints_sig.get_untracked() {
                hints_sig.set(hints);
                cache.update(|r| *r += 1);
            }
        });
        std::thread::spawn(move || {
            let hints = client.inlay_hints(&uri, end_line).unwrap_or_default();
            send(hints);
        });
    }

    pub fn request_inlay_hints_active(&self) {
        if let Some(id) = self.focused_active_id() {
            self.request_inlay_hints(id);
        }
    }

    pub fn request_outline(&self) {
        let outline = self.outline;
        let Some(buf) = self.active_buffer() else {
            outline.set(Vec::new());
            return;
        };
        let (Some(client), Some(uri)) = (self.lsp_for_active(), buf.uri.clone()) else {
            outline.set(Vec::new());
            return;
        };
        if self.lsp_language_id(buf.file.language).is_none() {
            outline.set(Vec::new());
            return;
        }
        self.spawn_bg(
            move || {
                client
                    .document_symbols(&uri)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(name, kind, line, ch, depth)| OutlineItem {
                        name,
                        kind,
                        line,
                        char: ch,
                        depth,
                    })
                    .collect::<Vec<_>>()
            },
            move |items| outline.set(items),
        );
    }
}
