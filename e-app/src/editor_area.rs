//! The central editor area: one or two panes, each showing its active buffer.
//! Hidden editors stay alive so every tab keeps its cursor and scroll.

use std::rc::Rc;

use floem::event::{EventListener, EventPropagation};
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::editor::command::{Command, CommandExecuted};
use floem::views::editor::core::command::{EditCommand, MoveCommand};
use floem::views::editor::core::cursor::{Cursor, CursorMode};
use floem::views::editor::core::selection::Selection;
use floem::views::editor::keypress::default_key_handler;
use floem::views::editor::keypress::key::KeyInput;
use floem::views::editor::text::Document;
use floem::views::{
    container, dyn_container, dyn_stack, label, stack, text_editor_keys, Decorators,
};
use floem::IntoView;

use crate::app::handle_shortcut;

use crate::state::AppState;
use crate::styling::SyntaxStyling;
use crate::theme;

/// One shortcut row in the welcome cheatsheet: key chip + description.
fn cheat(key: &'static str, desc: &'static str) -> impl IntoView {
    stack((
        label(move || key.to_string()).style(|s| {
            s.width(64.0)
                .justify_end()
                .font_family("monospace".to_string())
                .font_size(12.0)
                .color(theme::fg())
        }),
        label(move || desc.to_string()).style(|s| s.color(theme::fg_dim()).font_size(13.0)),
    ))
    .style(|s| s.items_center().gap(16.0).height(26.0))
}

/// The empty-state welcome screen with the key shortcuts.
fn welcome() -> impl IntoView {
    // The shortcut rows form a left-aligned block...
    let col1 = stack((
        cheat("⌘P", "Find file"),
        cheat("⌘E", "Recent files"),
        cheat("⌘O", "Open folder/project"),
        cheat("⇧⌘P", "Command palette"),
        cheat("⇧⌘F", "Search in files"),
        cheat("⇧⌘O", "Go to symbol"),
        cheat("⌘F", "Find in file"),
        cheat("⌥⌘F", "Replace in file"),
        cheat("⌃G", "Go to line"),
    ))
    .style(|s| s.flex_col().items_start().gap(8.0));

    let col2 = stack((
        cheat("⌘/", "Toggle comment"),
        cheat("⌘T", "Toggle terminal"),
        cheat("⌘L", "Toggle agent panel"),
        cheat("⌘1", "Toggle sidebar"),
        cheat("⌘2", "Source control"),
        cheat("⌘\\", "Split editor"),
        cheat("F12", "Go to definition"),
        cheat("F8", "Light / dark theme"),
    ))
    .style(|s| s.flex_col().items_start().gap(8.0));

    let cheats = stack((col1, col2)).style(|s| s.flex_row().gap(40.0));

    // ...which is centred as a whole, with the title centred above it.
    stack((
        label(|| "e".to_string())
            .style(|s| s.font_size(44.0).color(theme::fg()).margin_bottom(4.0)),
        label(|| "The editor for the rest of us".to_string())
            .style(|s| s.color(theme::fg_dim()).font_size(13.0).margin_bottom(22.0)),
        cheats,
    ))
    .style(|s| s.flex_col().items_center())
}

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

            let te = text_editor_keys("", move |editor_sig, kp, mods| {
                // App shortcuts first (the editor otherwise swallows every key).
                if let KeyInput::Keyboard(key, _) = &kp.key {
                    if handle_shortcut(state, key, mods) {
                        return CommandExecuted::Yes;
                    }
                    // Auto-pairing for plain typed brackets/quotes.
                    if !mods.meta() && !mods.control() && !mods.alt() {
                        match key {
                            floem::keyboard::Key::Character(s) if s.chars().count() == 1 => {
                                let ch = s.chars().next().unwrap();
                                if matches!(ch, '(' | ')' | '[' | ']' | '{' | '}' | '"' | '\'' | '`')
                                    && state.handle_autopair(ch)
                                {
                                    return CommandExecuted::Yes;
                                }
                            }
                            floem::keyboard::Key::Named(floem::keyboard::NamedKey::Backspace) => {
                                if state.handle_autopair_backspace() {
                                    return CommandExecuted::Yes;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                default_key_handler(editor_sig)(kp, mods)
            })
            .use_doc(Rc::new(crate::hints_doc::HintsDoc::new(b.doc.clone(), b.inlay_hints))
                as Rc<dyn Document>)
            .styling(SyntaxStyling::new(
                b.highlights.clone(),
                b.diag_lines.clone(),
                b.git_marks.clone(),
                b.find_marks.clone(),
                b.bracket_marks.clone(),
                state.font_size,
                state.settings.tab_width,
            ))
            .editor_style(move |s| {
                let wrap = if state.word_wrap.get() {
                    floem::views::editor::text::WrapMethod::EditorWidth
                } else {
                    floem::views::editor::text::WrapMethod::None
                };
                theme::editor_style(s)
                    .indent_guide(state.settings.indent_guides)
                    .wrap_method(wrap)
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

            // Give this editor keyboard focus whenever its buffer becomes the
            // active one (e.g. ⌘N, opening a file, switching tabs), so you can
            // type immediately without clicking into it first. We focus the
            // inner, keyboard-navigable editor view (`editor_view_id`), which is
            // set once that view is built.
            {
                let editor_handle = editor_handle.clone();
                floem::reactive::create_effect(move |_| {
                    let vid = editor_handle.editor_view_id.get();
                    if active_sig.get() == Some(id) && !state.any_overlay_open() {
                        if let Some(vid) = vid {
                            floem::action::exec_after(
                                std::time::Duration::from_millis(0),
                                move |_| {
                                    // Re-check at fire time: a palette may have
                                    // opened since, and must keep focus.
                                    if !state.any_overlay_open() {
                                        vid.request_focus();
                                    }
                                },
                            );
                        }
                    }
                });
            }

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
                let placeholder = container(welcome()).style(move |s| {
                    let s = s
                        .absolute()
                        .inset(0.0)
                        .size_full()
                        .items_center()
                        .justify_center();
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
