//! Everyday editing commands: line operations (move / duplicate / delete /
//! indent), comment toggling, and go-to-line. Line operations delegate to
//! Floem's built-in editor commands; comment toggling is implemented here
//! because Floem hard-codes an empty comment token.

use floem::keyboard::Modifiers;
use floem::reactive::{SignalGet, SignalUpdate};
use floem::views::editor::command::Command;
use floem::views::editor::core::command::EditCommand;
use floem::views::editor::core::cursor::{Cursor, CursorMode};
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::selection::Selection;
use floem::views::editor::text::Document;

use floem::reactive::RwSignal;

use crate::state::{line_of, line_starts, AppState};
use crate::theme;

/// State for the go-to-line prompt.
#[derive(Clone, Copy)]
pub struct GotoState {
    pub open: RwSignal<bool>,
    pub query: RwSignal<String>,
}

impl GotoState {
    pub fn new() -> Self {
        Self {
            open: RwSignal::new(false),
            query: RwSignal::new(String::new()),
        }
    }
}

impl AppState {
    /// Run a built-in editor edit command on the active buffer.
    fn run_edit(&self, cmd: EditCommand) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(editor) = buf.editor.get_untracked() else {
            return;
        };
        buf.doc
            .run_command(&editor, &Command::Edit(cmd), None, Modifiers::empty());
    }

    pub fn move_line_up(&self) {
        self.run_edit(EditCommand::MoveLineUp);
    }
    pub fn move_line_down(&self) {
        self.run_edit(EditCommand::MoveLineDown);
    }
    pub fn duplicate_line(&self) {
        self.run_edit(EditCommand::DuplicateLineDown);
    }
    pub fn delete_line(&self) {
        self.run_edit(EditCommand::DeleteLine);
    }
    pub fn indent_lines(&self) {
        self.run_edit(EditCommand::IndentLine);
    }
    pub fn outdent_lines(&self) {
        self.run_edit(EditCommand::OutdentLine);
    }

    /// Toggle line comments across the selected lines. Comments when at least
    /// one selected non-blank line is uncommented, otherwise uncomments.
    pub fn toggle_comment(&self) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(token) = buf.file.language.line_comment() else {
            return;
        };
        let Some(editor) = buf.editor.get_untracked() else {
            return;
        };

        let text = buf.doc.text().to_string();
        let starts = line_starts(&text);
        if starts.is_empty() {
            return;
        }

        let cursor = editor.cursor.get_untracked();
        let (min, max) = match cursor.mode.clone() {
            CursorMode::Insert(sel) => {
                let regions = sel.regions();
                let min = regions.iter().map(|r| r.min()).min().unwrap_or(0);
                let max = regions.iter().map(|r| r.max()).max().unwrap_or(0);
                (min, max)
            }
            _ => {
                let o = cursor.offset();
                (o, o)
            }
        };

        let first = line_of(&starts, min);
        let last = line_of(&starts, max.min(text.len().saturating_sub(1)));

        let line_end = |l: usize| -> usize {
            if l + 1 < starts.len() {
                starts[l + 1] - 1
            } else {
                text.len()
            }
        };

        // Decide direction: comment unless every non-blank line is commented.
        let mut all_commented = true;
        for l in first..=last {
            let line = &text[starts[l]..line_end(l)];
            let trimmed = line.trim_start();
            if trimmed.is_empty() {
                continue;
            }
            if !trimmed.starts_with(token) {
                all_commented = false;
                break;
            }
        }

        let mut edits: Vec<(Selection, String)> = Vec::new();
        for l in first..=last {
            let ls = starts[l];
            let line = &text[ls..line_end(l)];
            let trimmed = line.trim_start();
            if trimmed.is_empty() {
                continue;
            }
            let indent = line.len() - trimmed.len();
            let tok_at = ls + indent;
            if all_commented {
                // Remove the token and a single following space, if present.
                let after = tok_at + token.len();
                let rest = &text[after..line_end(l)];
                let end = if rest.starts_with(' ') { after + 1 } else { after };
                edits.push((Selection::region(tok_at, end), String::new()));
            } else {
                edits.push((Selection::caret(tok_at), format!("{token} ")));
            }
        }

        if edits.is_empty() {
            return;
        }
        let mut it = edits.iter().map(|(s, t)| (s.clone(), t.as_str()));
        buf.doc.edit(&mut it, EditType::ToggleComment);
    }

    /// Move the caret to the given 1-based line (and optional 1-based column).
    pub fn goto_line(&self, line: usize, col: usize) {
        let Some(buf) = self.active_buffer() else {
            return;
        };
        let Some(editor) = buf.editor.get_untracked() else {
            return;
        };
        let text = buf.doc.text().to_string();
        let starts = line_starts(&text);
        if starts.is_empty() {
            return;
        }
        let li = line.saturating_sub(1).min(starts.len() - 1);
        let line_start = starts[li];
        let line_end = if li + 1 < starts.len() {
            starts[li + 1] - 1
        } else {
            text.len()
        };
        let offset = (line_start + col.saturating_sub(1)).min(line_end);
        editor
            .cursor
            .set(Cursor::new(CursorMode::Insert(Selection::caret(offset)), None, None));
    }

    /// Open the go-to-line prompt.
    pub fn open_goto_line(&self) {
        self.goto.query.set(String::new());
        self.goto.open.set(true);
    }

    pub fn close_goto_line(&self) {
        self.goto.open.set(false);
    }

    /// Parse the current go-to-line query (`line` or `line:col`) and jump.
    pub fn confirm_goto_line(&self) {
        let q = self.goto.query.get_untracked();
        self.goto.open.set(false);
        let mut parts = q.trim().splitn(2, ':');
        let Some(line) = parts.next().and_then(|s| s.trim().parse::<usize>().ok()) else {
            return;
        };
        let col = parts.next().and_then(|s| s.trim().parse::<usize>().ok()).unwrap_or(1);
        self.goto_line(line, col);
    }
}

/// The go-to-line prompt overlay (⌃G): a small input centred near the top.
pub fn goto_bar(state: AppState) -> impl floem::IntoView {
    use floem::keyboard::{Key, NamedKey};
    use floem::views::{container, label, stack, text_input, Decorators};

    let goto = state.goto;
    let input = text_input(goto.query)
        .placeholder("Go to line  (line or line:col)")
        .on_enter(move || state.confirm_goto_line())
        .style(|s| {
            theme::input_colors(s)
                .width(260.0)
                .height(30.0)
                .padding_horiz(8.0)
                .border(1.0)
                .border_radius(4.0)
        })
        .request_focus(move || {
            goto.open.get();
        })
        .on_key_down(Key::Named(NamedKey::Escape), |_| true, move |_| {
            state.close_goto_line()
        });

    let box_ = stack((
        label(|| "Go to line".to_string()).style(|s| s.color(theme::fg_dim()).font_size(12.0)),
        input,
    ))
    .style(|s| {
        s.flex_col()
            .gap(8.0)
            .padding(14.0)
            .background(theme::bg_panel())
            .border(1.0)
            .border_color(theme::border())
            .border_radius(8.0)
    });

    container(box_)
        .style(move |s| {
            let s = s.absolute().inset(0.0).size_full().justify_center().padding_top(90.0);
            if goto.open.get() {
                s
            } else {
                s.hide()
            }
        })
        .on_click_stop(move |_| state.close_goto_line())
}
