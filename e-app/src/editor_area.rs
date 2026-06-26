//! The central editor area: one or two panes, each showing its active buffer.
//! Hidden editors stay alive so every tab keeps its cursor and scroll.

use std::rc::Rc;

use floem::event::{EventListener, EventPropagation};
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::command::{Command, CommandExecuted};
use floem::views::editor::core::command::{EditCommand, MoveCommand};
use floem::views::editor::core::cursor::{Cursor, CursorMode};
use floem::views::editor::core::selection::Selection;
use floem::views::editor::text::Document;
use floem::views::{container, dyn_container, dyn_stack, label, stack, text_editor, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::styling::SyntaxStyling;
use crate::theme;

/// One pane: a stack of all buffers, only this pane's active one visible.
fn pane(state: AppState, pane_idx: u8) -> impl IntoView {
    let active_sig: RwSignal<Option<u64>> = if pane_idx == 1 {
        state.active2
    } else {
        state.active
    };

    dyn_stack(
        move || state.buffers.get(),
        |b| b.id,
        move |b| {
            let id = b.id;
            let win_origin = b.win_origin;

            let te = text_editor("")
                .use_doc(b.doc.clone() as Rc<dyn Document>)
                .styling(SyntaxStyling::new(
                    b.highlights.clone(),
                    b.diag_lines.clone(),
                    b.git_marks.clone(),
                    b.find_marks.clone(),
                    b.bracket_marks.clone(),
                    state.settings.font_size,
                    state.settings.tab_width,
                ))
                .editor_style(move |s| {
                    theme::editor_style(s).indent_guide(state.settings.indent_guides)
                })
                .style(|s| s.size_full())
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

            let editor_handle = te.editor().clone();
            b.editor.set(Some(editor_handle.clone()));

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
                .on_event(EventListener::PointerDown, move |_| {
                    state.focused.set(pane_idx);
                    EventPropagation::Continue
                })
                .style(move |s| {
                    if active_sig.get() == Some(id) {
                        s.size_full()
                    } else {
                        s.hide()
                    }
                })
        },
    )
    .style(|s| s.size_full())
}

pub fn editor_area(state: AppState) -> impl IntoView {
    dyn_container(
        move || state.split.get(),
        move |split| {
            if split {
                stack((
                    pane(state, 0).style(|s| {
                        s.flex_grow(1.0)
                            .height_full()
                            .border_right(1.0)
                            .border_color(theme::border())
                    }),
                    pane(state, 1).style(|s| s.flex_grow(1.0).height_full()),
                ))
                .style(|s| s.flex_row().size_full().background(theme::bg()))
                .into_any()
            } else {
                let placeholder =
                    container(label(|| "No file open — press ⌘P to find a file".to_string()))
                        .style(move |s| {
                            let s = s
                                .size_full()
                                .items_center()
                                .justify_center()
                                .color(theme::fg_dim());
                            if state.buffers.with(|b| b.is_empty()) {
                                s
                            } else {
                                s.hide()
                            }
                        });
                stack((placeholder, pane(state, 0)))
                    .style(|s| s.size_full().background(theme::bg()))
                    .into_any()
            }
        },
    )
    .style(|s| s.size_full())
}
