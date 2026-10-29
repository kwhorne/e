//! A click on a change bar (or a removal's wedge) in the gutter opens a peek
//! at that hunk beside the line: what the lines were in the last commit, with
//! **Stage** — the hunk alone into the index, the rest of the file as it is —
//! and **Revert** — the lines in the editor back to the commit, as one edit
//! that undo takes back. A hunk already in the index says so, with
//! **Unstage** to take it out again.

use std::path::PathBuf;

use e_core::git::{self, Hunk};
use floem::kurbo::Point;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::core::cursor::CursorAffinity;
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::selection::Selection;
use floem::views::editor::text::Document;
use floem::views::{dyn_stack, label, stack, Decorators};
use floem::IntoView;

use crate::state::{AppState, Buffer};
use crate::theme;

/// The longest run of old lines the peek shows; the rest is counted.
const MAX_LINES: usize = 14;

/// An open peek at one hunk.
#[derive(Clone, Debug, PartialEq)]
pub struct HunkPeek {
    pub path: PathBuf,
    /// The line clicked (0-based), which finds the hunk again when acting.
    pub line: usize,
    pub hunk: Hunk,
    /// The lines as they were in the last commit, newline-free.
    pub old_lines: Vec<String>,
    /// Where it opens, in window coordinates: under the clicked line.
    pub anchor: Point,
    /// Whether the hunk is in the index — looked up as the peek opens, and
    /// set by Stage and Unstage.
    pub staged: bool,
}

impl AppState {
    fn buffer_for(&self, path: &std::path::Path) -> Option<Buffer> {
        self.buffers.with_untracked(|bs| {
            bs.iter()
                .find(|b| b.file.path.as_deref() == Some(path))
                .cloned()
        })
    }

    /// Open the peek for the hunk at `line` of the file at `path`. `false`
    /// when there is none — the line didn't change.
    pub(crate) fn peek_hunk(&self, path: &std::path::Path, line: usize) -> bool {
        let Some(buf) = self.buffer_for(path) else {
            return false;
        };
        let Some(head) = buf.head_text.borrow().clone() else {
            return false;
        };
        let text = buf.doc.text().to_string();
        let Some(hunk) = git::hunk_at(&head, &text, line) else {
            return false;
        };
        // Already open on it — by a click, before a command that acts on it:
        // keep what it knows.
        let open = self
            .hunk_peek
            .with_untracked(|p| p.as_ref().is_some_and(|p| p.path == path && p.hunk == hunk));
        if open {
            self.hunk_peek.update(|p| {
                if let Some(p) = p.as_mut() {
                    p.line = line;
                }
            });
            return true;
        }
        let old_lines = head[git::line_bytes(&head, hunk.old.clone())]
            .lines()
            .map(String::from)
            .collect();
        let anchor = buf
            .editor
            .get_untracked()
            .map(|editor| {
                let offset = editor.offset_of_line(line);
                let (_, below) = editor.points_of_offset(offset, CursorAffinity::Backward);
                let vp = editor.viewport.get_untracked();
                let win = buf.win_origin.get_untracked();
                Point::new(win.x, win.y + below.y - vp.y0)
            })
            .unwrap_or_default();
        self.hunk_peek.set(Some(HunkPeek {
            path: path.to_path_buf(),
            line,
            hunk,
            old_lines,
            anchor,
            staged: false,
        }));
        // Staged already? The index is read off the UI thread.
        let path = path.to_path_buf();
        let state = *self;
        self.spawn_bg(
            {
                let path = path.clone();
                move || git::index_text(&path).is_some_and(|i| git::hunk_staged(&i, &text, line))
            },
            move |staged: bool| state.mark_peek_staged(&path, line, staged),
        );
        true
    }

    /// Note on the open peek at `line` of `path` whether its hunk is staged.
    fn mark_peek_staged(&self, path: &std::path::Path, line: usize, staged: bool) {
        let open = self.hunk_peek.with_untracked(|p| {
            p.as_ref()
                .is_some_and(|p| p.path == path && p.line == line && p.staged != staged)
        });
        if open {
            self.hunk_peek.update(|p| {
                if let Some(p) = p.as_mut() {
                    p.staged = staged;
                }
            });
        }
    }

    /// Open the peek for the change at the caret, as a click on its bar
    /// would. `false` when the caret's line didn't change.
    pub fn peek_hunk_at_caret(&self) -> bool {
        let Some(buf) = self.active_buffer() else {
            return false;
        };
        let (Some(path), Some(editor)) = (buf.file.path.clone(), buf.editor.get_untracked()) else {
            return false;
        };
        let (line, _) = editor.offset_to_line_col(editor.cursor.get_untracked().offset());
        let found = self.peek_hunk(&path, line);
        if !found {
            Self::notify("No change at the caret");
        }
        found
    }

    pub fn close_hunk_peek(&self) {
        if self.hunk_peek.with_untracked(Option::is_some) {
            self.hunk_peek.set(None);
        }
    }

    /// Give the keyboard back to the peeked file's editor — after a click on
    /// the peek took it, so ⌘Z or typing goes where the change is.
    fn refocus_peeked(&self, path: &std::path::Path) {
        let vid = self
            .buffer_for(path)
            .and_then(|b| b.editor.get_untracked())
            .and_then(|ed| ed.editor_view_id.get_untracked());
        if let Some(vid) = vid {
            vid.request_focus();
        }
    }

    /// Put the peeked hunk's lines back as they were in the last commit.
    pub fn revert_hunk(&self) {
        let Some(peek) = self.hunk_peek.get_untracked() else {
            return;
        };
        self.close_hunk_peek();
        let Some(buf) = self.buffer_for(&peek.path) else {
            return;
        };
        let Some(head) = buf.head_text.borrow().clone() else {
            return;
        };
        // Found again: the text may have changed since the peek opened.
        let text = buf.doc.text().to_string();
        let Some(h) = git::hunk_at(&head, &text, peek.line) else {
            return;
        };
        let at = git::line_bytes(&text, h.new.clone());
        let old = &head[git::line_bytes(&head, h.old.clone())];
        let mut edit = std::iter::once((Selection::region(at.start, at.end), old));
        buf.doc.edit(&mut edit, EditType::Other);
        self.refocus_peeked(&peek.path);
    }

    /// Stage the peeked hunk: the index gets it, and nothing else of the
    /// file's changes. The editor's text is what is staged, saved or not.
    pub fn stage_hunk(&self) {
        let Some(peek) = self.hunk_peek.get_untracked() else {
            return;
        };
        let Some(buf) = self.buffer_for(&peek.path) else {
            return;
        };
        let text = buf.doc.text().to_string();
        let path = peek.path.clone();
        let state = *self;
        self.spawn_bg(
            move || {
                // A file the index doesn't have yet starts from nothing.
                let index = git::index_text(&path).unwrap_or_default();
                let h = git::hunk_at(&index, &text, peek.line)
                    .ok_or("This change is already staged")?;
                git::stage_text(&path, &git::apply_hunk(&index, &text, &h))
            },
            move |res: Result<(), String>| {
                match res {
                    Ok(()) => state.mark_peek_staged(&peek.path, peek.line, true),
                    Err(e) => AppState::notify(&format!("Couldn't stage the change: {e}")),
                }
                state.refocus_peeked(&peek.path);
                state.refresh_git_status();
            },
        );
    }

    /// Take the peeked hunk out of the index again: those lines go back to
    /// the last commit's there, and the rest of what is staged stays.
    pub fn unstage_hunk(&self) {
        let Some(peek) = self.hunk_peek.get_untracked() else {
            return;
        };
        let Some(buf) = self.buffer_for(&peek.path) else {
            return;
        };
        let Some(head) = buf.head_text.borrow().clone() else {
            return;
        };
        let text = buf.doc.text().to_string();
        let path = peek.path.clone();
        let state = *self;
        self.spawn_bg(
            move || {
                let index = git::index_text(&path).ok_or("The file isn't in the index")?;
                let back = git::unstage_hunk(&head, &index, &text, peek.line)
                    .ok_or("This change isn't staged")?;
                git::stage_text(&path, &back)
            },
            move |res: Result<(), String>| {
                match res {
                    Ok(()) => state.mark_peek_staged(&peek.path, peek.line, false),
                    Err(e) => AppState::notify(&format!("Couldn't unstage the change: {e}")),
                }
                state.refocus_peeked(&peek.path);
                state.refresh_git_status();
            },
        );
    }

    /// Notice a commit or checkout made anywhere — in a terminal, by an agent —
    /// by the reflog's modification time, and bring the change marks up to
    /// date. A `stat` per call; called on the idle tick.
    pub fn check_head_moved(&self) {
        let Some(repo) = self.git_root.get_untracked() else {
            return;
        };
        let reflog = repo.join(".git/logs/HEAD");
        let modified = std::fs::metadata(&reflog).and_then(|m| m.modified()).ok();
        let seen = self.head_moved.get_untracked();
        if seen != modified {
            self.head_moved.set(modified);
            if seen.is_some() {
                self.refresh_git_status();
                // A checkout, pull or merge brings other migrations along.
                if git::last_head_move(&reflog).is_some_and(|m| !git::is_own_commit(&m)) {
                    self.check_migration_drift();
                }
            }
        }
    }

    /// Read every open file's last commit again and redraw its change marks:
    /// after a commit or a checkout, HEAD is something else.
    pub(crate) fn refresh_head_texts(&self) {
        let buffers = self.buffers.get_untracked();
        for b in buffers {
            let Some(path) = b.file.path.clone() else {
                continue;
            };
            self.spawn_bg(
                move || git::head_text(&path),
                move |head: Option<String>| {
                    if *b.head_text.borrow() == head {
                        return;
                    }
                    let text = b.doc.text().to_string();
                    *b.git_marks.borrow_mut() = match &head {
                        Some(h) => {
                            let lines = text.split_inclusive('\n').count().max(1);
                            git::marks(h, &text, lines)
                        }
                        None => Vec::new(),
                    };
                    *b.head_text.borrow_mut() = head;
                    b.doc.cache_rev().update(|r| *r += 1);
                },
            );
        }
    }
}

/// The peek, floating under the line that was clicked.
pub fn hunk_peek_view(state: AppState) -> impl IntoView {
    let peek = state.hunk_peek;
    let button = |text: &'static str| {
        label(move || text.to_string()).style(|s| {
            s.font_size(12.0)
                .padding_horiz(8.0)
                .padding_vert(2.0)
                .border_radius(4.0)
                .color(theme::accent())
                .cursor(floem::style::CursorStyle::Pointer)
                .hover(|s| s.background(theme::bg_hover()))
        })
    };
    let title = label(move || {
        peek.with(|p| {
            let Some(p) = p else {
                return String::new();
            };
            let (removed, added) = (p.hunk.old.len(), p.hunk.new.len());
            let what = match (removed, added) {
                (0, n) => format!("{n} line{} added", plural(n)),
                (n, 0) => format!("{n} line{} removed", plural(n)),
                (r, a) if r == a => format!("{r} line{} changed", plural(r)),
                (r, a) => format!("{r} line{} changed to {a}", plural(r)),
            };
            if p.staged {
                format!("{what} · staged")
            } else {
                what
            }
        })
    })
    .style(|s| s.flex_grow(1.0_f32).font_size(12.0).color(theme::fg_dim()));
    let staged = move || peek.with(|p| p.as_ref().is_some_and(|p| p.staged));
    let stage = button("Stage")
        .on_click_stop(move |_| state.stage_hunk())
        .style(move |s| s.apply_if(staged(), |s| s.hide()));
    let unstage = button("Unstage")
        .on_click_stop(move |_| state.unstage_hunk())
        .style(move |s| s.apply_if(!staged(), |s| s.hide()));
    let revert = button("Revert").on_click_stop(move |_| state.revert_hunk());
    let close = button("×").on_click_stop(move |_| {
        if let Some(p) = state.hunk_peek.get_untracked() {
            state.close_hunk_peek();
            state.refocus_peeked(&p.path);
        }
    });
    let header = stack((title, stage, unstage, revert, close)).style(|s| {
        s.items_center()
            .gap(2.0)
            .padding_horiz(8.0)
            .padding_vert(4.0)
    });
    let lines = dyn_stack(
        move || {
            peek.with(|p| {
                let Some(p) = p else {
                    return Vec::new();
                };
                let mut rows: Vec<String> = p
                    .old_lines
                    .iter()
                    .take(MAX_LINES)
                    .map(|l| format!("− {l}"))
                    .collect();
                if p.old_lines.len() > MAX_LINES {
                    rows.push(format!("  … {} more", p.old_lines.len() - MAX_LINES));
                }
                rows.into_iter().enumerate().collect()
            })
        },
        |(i, _)| *i,
        move |(_, row)| {
            label(move || row.clone()).style(|s| {
                s.font_family(theme::mono_family())
                    .font_size(12.5)
                    .line_height(1.4)
                    .padding_horiz(10.0)
                    .color(theme::error())
                    .background(theme::error().multiply_alpha(0.13))
                    .width_full()
            })
        },
    )
    .style(|s| s.flex_col().width_full());

    stack((header, lines))
        .style(move |s| {
            let anchor = peek
                .with(|p| p.as_ref().map(|p| p.anchor))
                .unwrap_or_default();
            let s = s
                .absolute()
                .flex_col()
                .items_stretch()
                .inset_left(anchor.x)
                .inset_top(anchor.y + 2.0)
                .width(560.0)
                .padding_bottom(4.0)
                .background(theme::bg_panel())
                .border(1.0)
                .border_color(theme::border())
                .border_radius(6.0);
            if peek.with(Option::is_some) {
                s
            } else {
                s.hide()
            }
        })
        .on_event_stop(floem::event::EventListener::PointerDown, |_| {})
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}
