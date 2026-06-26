//! Application entry point and root view.

use std::path::PathBuf;

use floem::{
    keyboard::Key,
    reactive::{RwSignal, SignalGet, SignalUpdate},
    views::{
        editor::text::{default_dark_color, SimpleStyling},
        stack, text_editor, Decorators,
    },
    IntoView,
};

use e_core::buffer::{self, FileInfo};

use crate::status::status_bar;

/// Launch the editor.
pub fn launch() {
    floem::launch(app_view);
}

fn app_view() -> impl IntoView {
    // Milepæl 0: a single optional file argument.
    let path: Option<PathBuf> = std::env::args().nth(1).map(PathBuf::from);

    let file = match &path {
        Some(p) => FileInfo::for_path(p),
        None => FileInfo::scratch(),
    };
    let initial = match &path {
        Some(p) => buffer::read_to_string(p).unwrap_or_default(),
        None => String::new(),
    };

    let dirty = RwSignal::new(false);
    let language = file.language;
    let name = file.display_name();

    let editor = text_editor(initial)
        .styling(SimpleStyling::new())
        .editor_style(default_dark_color)
        .style(|s| s.size_full())
        .update(move |_| dirty.set(true));

    // Grab the document handle so Cmd/Ctrl+S can read the current text.
    let doc = editor.doc();

    let title_name = name.clone();
    let save_path = path.clone();

    stack((editor, status_bar(name, language, dirty)))
        .style(|s| s.flex_col().size_full())
        .window_title(move || {
            let mark = if dirty.get() { "● " } else { "" };
            format!("{mark}{title_name} — e")
        })
        .on_key_down(
            Key::Character("s".into()),
            |m| m.meta() || m.control(),
            move |_| {
                let Some(p) = &save_path else {
                    eprintln!("e: no file path — save-as is not implemented yet");
                    return;
                };
                let text = doc.text().to_string();
                match buffer::write(p, &text) {
                    Ok(()) => {
                        dirty.set(false);
                        eprintln!("e: saved {}", p.display());
                    }
                    Err(e) => eprintln!("e: save failed: {e:#}"),
                }
            },
        )
}
