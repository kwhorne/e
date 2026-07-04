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
use floem::views::editor::command::CommandExecuted;
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

    text_editor_keys("", move |editor_sig, kp, mods| {
        if let KeyInput::Keyboard(key, _) = &kp.key {
            // ⌘/Ctrl+Enter runs the console (intercepted before app shortcuts so
            // it doesn't hit the PHP "run SQL under cursor" binding).
            if matches!(
                key,
                floem::keyboard::Key::Named(floem::keyboard::NamedKey::Enter)
            ) && (mods.meta() || mods.control())
            {
                state.db_run_query();
                return CommandExecuted::Yes;
            }
            if handle_shortcut(state, key, mods) {
                return CommandExecuted::Yes;
            }
        }
        default_key_handler(editor_sig)(kp, mods)
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
    .style(|s| {
        s.size_full()
            .font_family("monospace".to_string())
            .font_size(13.0)
            .padding_horiz(10.0)
            .padding_vert(8.0)
    })
}
