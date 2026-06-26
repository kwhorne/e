//! The central editor area: one live editor per open buffer, only the active
//! one visible. Hidden editors stay alive in the view tree, so each tab keeps
//! its own cursor and scroll position.

use std::rc::Rc;

use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::command::{Command, CommandExecuted};
use floem::views::editor::core::command::{EditCommand, MoveCommand};
use floem::views::editor::core::cursor::{Cursor, CursorMode};
use floem::views::editor::core::selection::Selection;
use floem::views::editor::text::{default_dark_color, Document};
use floem::views::{container, dyn_stack, label, stack, text_editor, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::styling::SyntaxStyling;
use crate::theme;

pub fn editor_area(state: AppState) -> impl IntoView {
    let editors = dyn_stack(
        move || state.buffers.get(),
        |b| b.id,
        move |b| {
            let id = b.id;
            let active = state.active;

            let win_origin = b.win_origin;
            let te = text_editor("")
                .use_doc(b.doc.clone() as Rc<dyn Document>)
                .styling(SyntaxStyling::new(b.highlights.clone(), b.diag_lines.clone()))
                .editor_style(default_dark_color)
                .style(|s| s.size_full())
                // Intercept keys while the completion popup is open.
                .pre_command(move |pre| {
                    if state.completion.open.get_untracked() {
                        match pre.cmd {
                            Command::Move(MoveCommand::Down) => {
                                state.move_completion(1);
                                return CommandExecuted::Yes;
                            }
                            Command::Move(MoveCommand::Up) => {
                                state.move_completion(-1);
                                return CommandExecuted::Yes;
                            }
                            Command::Edit(EditCommand::InsertNewLine)
                            | Command::Edit(EditCommand::InsertTab) => {
                                if state.accept_completion() {
                                    return CommandExecuted::Yes;
                                }
                            }
                            _ => {}
                        }
                    }
                    CommandExecuted::No
                });

            // Hand the live editor to the buffer for cursor / position queries.
            let editor_handle = te.editor().clone();
            b.editor.set(Some(editor_handle.clone()));

            // Apply a pending go-to-definition jump now that the editor exists.
            if let Some((l, c)) = b.pending_goto.get_untracked() {
                let offset = editor_handle.offset_of_line_col(l, c);
                editor_handle.cursor.set(Cursor::new(
                    CursorMode::Insert(Selection::caret(offset)),
                    None,
                    None,
                ));
                b.pending_goto.set(None);
            }

            container(te)
                .on_move(move |point| win_origin.set(point))
                .style(move |s| {
                    if active.get() == Some(id) {
                        s.size_full()
                    } else {
                        s.hide()
                    }
                })
        },
    )
    .style(|s| s.size_full());

    // Empty-state shown when no buffer is open.
    let placeholder = container(label(|| "No file open — press ⌘P to find a file".to_string()))
        .style(|s| {
            s.size_full()
                .items_center()
                .justify_center()
                .color(theme::FG_DIM)
        });

    stack((
        placeholder.style(move |s| {
            if state.buffers.with(|b| b.is_empty()) {
                s.size_full()
            } else {
                s.hide()
            }
        }),
        editors,
    ))
    .style(|s| s.size_full().background(theme::BG))
}
