//! A breadcrumb bar: the active file's path plus the symbol at the cursor.

use floem::reactive::SignalGet;
use floem::views::{label, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;

pub fn breadcrumbs(state: AppState) -> impl IntoView {
    label(move || {
        let Some(buf) = state.active_buffer() else {
            return String::new();
        };

        // Relative path, shown as " › "-separated segments.
        let root = state.root.get();
        let path = buf
            .file
            .path
            .as_ref()
            .map(|p| {
                p.strip_prefix(&root)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .replace('/', "  ›  ")
            })
            .unwrap_or_else(|| buf.file.display_name());

        // Nearest enclosing symbol (deepest preceding outline entry).
        if let Some(editor) = buf.editor.get() {
            let line = editor.offset_to_line_col(editor.cursor.get().offset()).0;
            let outline = state.outline.get();
            if let Some(sym) = outline.iter().filter(|s| (s.line as usize) <= line).last() {
                return format!("{path}  ›  {}", sym.name);
            }
        }
        path
    })
    .style(|s| {
        s.height(24.0)
            .width_full()
            .items_center()
            .padding_horiz(12.0)
            .font_size(12.0)
            .color(theme::fg_dim())
            .background(theme::bg())
            .border_bottom(1.0)
            .border_color(theme::border())
    })
}
