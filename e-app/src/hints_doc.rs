//! A thin wrapper around `TextDocument` that injects LSP inlay hints as
//! phantom text, without changing how the rest of the app stores documents.
//!
//! Every `Document`/`DocumentPhantom` method delegates to the inner
//! `TextDocument` except `phantom_text`, which appends inlay hints. The layout
//! methods use the trait default — exactly as `TextDocument` does — so they
//! read our `phantom_text`.

use std::rc::Rc;

use floem::keyboard::Modifiers;
use floem::peniko::Color;
use floem::reactive::{RwSignal, SignalGet};
use floem::views::editor::command::{Command, CommandExecuted};
use floem::views::editor::core::buffer::rope_text::RopeText;
use floem::views::editor::core::xi_rope::Rope;
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::selection::Selection;
use floem::views::editor::id::EditorId;
use floem::views::editor::phantom_text::{PhantomText, PhantomTextKind, PhantomTextLine};
use floem::views::editor::text::{Document, DocumentPhantom, PreeditData};
use floem::views::editor::text_document::TextDocument;
use floem::views::editor::{Editor, EditorStyle};

fn hint_color() -> Color {
    Color::from_rgb8(0x6b, 0x73, 0x80)
}

pub struct HintsDoc {
    inner: Rc<TextDocument>,
    hints: RwSignal<Vec<(u32, u32, String)>>,
}

impl HintsDoc {
    pub fn new(inner: Rc<TextDocument>, hints: RwSignal<Vec<(u32, u32, String)>>) -> Self {
        Self { inner, hints }
    }
}

impl DocumentPhantom for HintsDoc {
    fn phantom_text(&self, edid: EditorId, styling: &EditorStyle, line: usize) -> PhantomTextLine {
        let mut pl = self.inner.phantom_text(edid, styling, line);
        let hints = self.hints.get_untracked();
        if !hints.is_empty() {
            let rope = self.inner.rope_text();
            let line_start = rope.offset_of_line(line);
            let line_end = rope.offset_of_line(line + 1);
            for (l, ch, label) in &hints {
                if *l as usize != line {
                    continue;
                }
                let off = (line_start + *ch as usize).min(line_end);
                let (_, col) = rope.offset_to_line_col(off);
                pl.text.push(PhantomText {
                    kind: PhantomTextKind::InlayHint,
                    col,
                    affinity: None,
                    text: label.clone(),
                    font_size: None,
                    fg: Some(hint_color()),
                    bg: None,
                    under_line: None,
                });
            }
            pl.text.sort_by_key(|p| p.col);
        }
        pl
    }

    fn has_multiline_phantom(&self, edid: EditorId, styling: &EditorStyle) -> bool {
        self.inner.has_multiline_phantom(edid, styling)
    }
}

impl Document for HintsDoc {
    fn text(&self) -> Rope {
        self.inner.text()
    }

    fn cache_rev(&self) -> RwSignal<u64> {
        self.inner.cache_rev()
    }

    fn preedit(&self) -> PreeditData {
        self.inner.preedit()
    }

    fn run_command(
        &self,
        ed: &Editor,
        cmd: &Command,
        count: Option<usize>,
        modifiers: Modifiers,
    ) -> CommandExecuted {
        self.inner.run_command(ed, cmd, count, modifiers)
    }

    fn receive_char(&self, ed: &Editor, c: &str) {
        self.inner.receive_char(ed, c)
    }

    fn edit(&self, iter: &mut dyn Iterator<Item = (Selection, &str)>, edit_type: EditType) {
        self.inner.edit(iter, edit_type)
    }
}
