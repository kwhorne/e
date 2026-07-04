//! The SQL console editor: a real code editor (syntax-highlighted SQL) for the
//! database panel's query box, replacing the old plain text input.
//!
//! The editor's backing [`TextDocument`] is stored on [`AppState`] so
//! programmatic SQL (browse queries, run-under-cursor, saved/history queries)
//! can be pushed in via `set_console_sql`; the editor's own edits mirror back
//! into the `db_query_text` signal that the run path reads.

use std::cell::RefCell;
use std::rc::Rc;

use floem::reactive::{SignalGet, SignalUpdate};
use floem::views::editor::command::{Command, CommandExecuted};
use floem::views::editor::core::command::{EditCommand, MoveCommand};
use floem::views::editor::keypress::default_key_handler;
use floem::views::editor::keypress::key::KeyInput;
use floem::views::editor::text::{Document, WrapMethod};
use floem::views::editor::text_document::TextDocument;
use floem::views::{text_editor_keys, Decorators};
use floem::IntoView;

use e_core::language::Language;

use crate::app::handle_shortcut;
use crate::state::AppState;
use crate::styling::{Highlights, SyntaxStyling};
use crate::theme;

/// Build the syntax-highlighted SQL console editor for the database panel.
pub fn sql_console(state: AppState) -> impl IntoView {
    let initial = state.db_query_text.get_untracked();

    // Dedicated SQL document (a real TextDocument, so the builder's `.update()`
    // and `.pre_command()` hooks attach correctly).
    let doc = Rc::new(TextDocument::new(state.cx, initial.clone()));
    state.db_console_doc.set(Some(doc.clone()));

    // Highlights recomputed on every edit; SyntaxStyling reads them per line.
    let highlights: Highlights = Rc::new(RefCell::new(e_core::syntax::highlight_lines(
        Language::Sql,
        &initial,
    )));

    let tab_width = state.settings.get_untracked().tab_width;

    let styling = SyntaxStyling::new(
        highlights.clone(),
        Default::default(),
        Default::default(),
        Default::default(),
        Default::default(),
        Default::default(),
        Default::default(),
        state.font_size,
        tab_width,
    );

    let hl = highlights.clone();
    let doc_for_update = doc.clone();

    let te = text_editor_keys("", move |editor_sig, kp, mods| {
        let comp = state.completion;
        let ours = comp.open.get_untracked()
            && comp.buffer_id.get_untracked() == Some(crate::completion_state::CONSOLE_COMP_ID);
        if let KeyInput::Keyboard(key, _) = &kp.key {
            use floem::keyboard::{Key, NamedKey};
            // ⌘/Ctrl+Enter runs the selection or the statement under the cursor;
            // add Shift to run the whole console. Intercepted before app shortcuts
            // so it doesn't hit the PHP "run SQL under cursor" binding.
            if matches!(key, Key::Named(NamedKey::Enter)) && (mods.meta() || mods.control()) {
                if mods.shift() {
                    state.db_run_query();
                } else {
                    state.run_console_under_cursor();
                }
                return CommandExecuted::Yes;
            }
            // Dismiss the completion popup on Escape.
            if ours && matches!(key, Key::Named(NamedKey::Escape)) {
                comp.open.set(false);
                return CommandExecuted::Yes;
            }
            if handle_shortcut(state, key, mods) {
                return CommandExecuted::Yes;
            }
        }
        let res = default_key_handler(editor_sig)(kp, mods);
        // After a character or backspace, (re)compute schema completion. Doing
        // it here (not in `.update`) means programmatic edits (browse queries)
        // never pop the completion list.
        if let KeyInput::Keyboard(key, _) = &kp.key {
            use floem::keyboard::{Key, NamedKey};
            if matches!(key, Key::Character(_) | Key::Named(NamedKey::Backspace)) {
                state.console_sql_completion();
            }
        }
        res
    })
    .use_doc(doc.clone() as Rc<dyn Document>)
    .styling(styling)
    .editor_style(|s| theme::editor_style(s).wrap_method(WrapMethod::EditorWidth))
    .update(move |_| {
        // Re-highlight and mirror the text back into the run signal.
        let text = doc_for_update.text().to_string();
        *hl.borrow_mut() = e_core::syntax::highlight_lines(Language::Sql, &text);
        doc_for_update.cache_rev().update(|r| *r += 1);
        state.db_query_text.set(text);
    })
    .pre_command(move |pre| {
        let comp = state.completion;
        if comp.open.get_untracked()
            && comp.buffer_id.get_untracked() == Some(crate::completion_state::CONSOLE_COMP_ID)
        {
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
                | Command::Edit(EditCommand::InsertTab)
                    if state.accept_console_completion() =>
                {
                    return CommandExecuted::Yes;
                }
                _ => {}
            }
        }
        CommandExecuted::No
    })
    .style(|s| {
        s.size_full()
            .font_family("monospace".to_string())
            .font_size(13.0)
            .padding_horiz(10.0)
            .padding_vert(8.0)
    });

    // Store the editor handle + track its window origin (for popup placement).
    state.db_console_editor.set(Some(te.editor().clone()));
    te.on_move(move |p| state.db_console_win.set(p))
}
