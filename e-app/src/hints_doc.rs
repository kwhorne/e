//! A thin wrapper around `TextDocument` that injects LSP inlay hints as
//! phantom text, without changing how the rest of the app stores documents.
//!
//! Every `Document`/`DocumentPhantom` method delegates to the inner
//! `TextDocument` except `phantom_text`, which appends inlay hints and
//! runtime lenses (see [`crate::runtime_lens`]). The layout
//! methods use the trait default — exactly as `TextDocument` does — so they
//! read our `phantom_text`.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use floem::keyboard::Modifiers;
use floem::peniko::Color;
use floem::reactive::{RwSignal, SignalGet, SignalWith};
use floem::views::editor::command::{Command, CommandExecuted};
use floem::views::editor::core::buffer::rope_text::RopeText;
use floem::views::editor::core::editor::EditType;
use floem::views::editor::core::selection::Selection;
use floem::views::editor::core::xi_rope::Rope;
use floem::views::editor::id::EditorId;
use floem::views::editor::phantom_text::{PhantomText, PhantomTextKind, PhantomTextLine};
use floem::views::editor::text::{Document, DocumentPhantom, PreeditData};
use floem::views::editor::text_document::TextDocument;
use floem::views::editor::{Editor, EditorStyle};

use crate::theme;

fn hint_color() -> Color {
    Color::from_rgb8(0x6b, 0x73, 0x80)
}

fn ghost_color() -> Color {
    theme::fg_dim()
}

fn lens_color(level: crate::runtime_lens::LensLevel) -> Color {
    use crate::runtime_lens::LensLevel;
    match level {
        LensLevel::Info => hint_color(),
        LensLevel::Warn => theme::warning(),
        LensLevel::Error => theme::error(),
    }
}

pub struct HintsDoc {
    inner: Rc<TextDocument>,
    hints: RwSignal<Vec<(u32, u32, String)>>,
    ghost: RwSignal<Option<crate::ghost::GhostText>>,
    lenses: RwSignal<Arc<Vec<crate::runtime_lens::Lens>>>,
    /// The buffer's file, which the lenses are looked up by.
    path: Option<PathBuf>,
    /// `(line, text)` shown dimmed at the end of the line.
    annotations: RwSignal<Vec<(usize, String)>>,
    /// `(line, text)` shown in red at the end of the line: exceptions.
    alerts: RwSignal<Vec<(usize, String)>>,
    /// Values of evaluated expressions, at the end of their lines.
    evals: RwSignal<Vec<crate::inline_eval::EvalMark>>,
}

impl HintsDoc {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        inner: Rc<TextDocument>,
        hints: RwSignal<Vec<(u32, u32, String)>>,
        ghost: RwSignal<Option<crate::ghost::GhostText>>,
        lenses: RwSignal<Arc<Vec<crate::runtime_lens::Lens>>>,
        path: Option<PathBuf>,
        annotations: RwSignal<Vec<(usize, String)>>,
        alerts: RwSignal<Vec<(usize, String)>>,
        evals: RwSignal<Vec<crate::inline_eval::EvalMark>>,
    ) -> Self {
        Self {
            inner,
            hints,
            ghost,
            lenses,
            path,
            annotations,
            alerts,
            evals,
        }
    }

    /// The value of the expression evaluated on `line`, at the end of it.
    fn eval_on(&self, line: usize) -> Option<PhantomText> {
        let (text, error) = self.evals.with_untracked(|m| {
            m.iter()
                .find(|e| e.line == line)
                .map(|e| (e.label.clone(), e.error))
        })?;
        let rope = self.inner.rope_text();
        let content = rope.line_content(line);
        Some(PhantomText {
            kind: PhantomTextKind::InlayHint,
            col: content.trim_end_matches(['\n', '\r']).len(),
            affinity: None,
            text: format!("   {text}"),
            font_size: None,
            fg: Some(if error { theme::error() } else { theme::info() }),
            bg: None,
            under_line: None,
        })
    }

    /// The exception on `line`, in red at the end of it.
    fn alert_on(&self, line: usize) -> Option<PhantomText> {
        let text = self
            .alerts
            .with_untracked(|a| a.iter().find(|(l, _)| *l == line).map(|(_, t)| t.clone()))?;
        let rope = self.inner.rope_text();
        let content = rope.line_content(line);
        Some(PhantomText {
            kind: PhantomTextKind::InlayHint,
            col: content.trim_end_matches(['\n', '\r']).len(),
            affinity: None,
            text: format!("   {text}"),
            font_size: None,
            fg: Some(theme::error()),
            bg: None,
            under_line: None,
        })
    }

    /// The annotation for `line`, at the end of it.
    fn annotation_on(&self, line: usize) -> Option<PhantomText> {
        let text = self.annotations.with_untracked(|a| {
            let on: Vec<&str> = a
                .iter()
                .filter(|(l, _)| *l == line)
                .map(|(_, t)| t.as_str())
                .collect();
            (!on.is_empty()).then(|| on.join("  ·  "))
        })?;
        let rope = self.inner.rope_text();
        let content = rope.line_content(line);
        Some(PhantomText {
            kind: PhantomTextKind::InlayHint,
            col: content.trim_end_matches(['\n', '\r']).len(),
            affinity: None,
            text: format!("   {text}"),
            font_size: None,
            fg: Some(hint_color()),
            bg: None,
            under_line: None,
        })
    }

    /// The runtime lens for `line`, at the end of it.
    fn lens_on(&self, line: usize) -> Option<PhantomText> {
        let path = self.path.as_ref()?;
        self.lenses.with_untracked(|ls| {
            if !ls.iter().any(|l| &l.file == path) {
                return None;
            }
            let rope = self.inner.rope_text();
            let content = rope.line_content(line);
            let anchor = crate::runtime_lens::LensAnchor::of_line(&content)?;
            let lens = ls.iter().find(|l| &l.file == path && l.anchor == anchor)?;
            Some(PhantomText {
                kind: PhantomTextKind::InlayHint,
                col: content.trim_end_matches(['\n', '\r']).len(),
                affinity: None,
                text: format!("   {}", lens.label),
                font_size: None,
                fg: Some(lens_color(lens.level)),
                bg: None,
                under_line: None,
            })
        })
    }
}

impl DocumentPhantom for HintsDoc {
    fn phantom_text(&self, edid: EditorId, styling: &EditorStyle, line: usize) -> PhantomTextLine {
        let mut pl = self.inner.phantom_text(edid, styling, line);
        // Called for every line laid out: look at the hints in place rather
        // than cloning the whole file's list (labels and all) each time.
        let added = self.hints.with_untracked(|hints| {
            let mut added = false;
            let rope = self.inner.rope_text();
            for (l, ch, label) in hints.iter().filter(|h| h.0 as usize == line) {
                let line_start = rope.offset_of_line(*l as usize);
                let line_end = rope.offset_of_line(line + 1);
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
                added = true;
            }
            added
        });
        if added {
            pl.text.sort_by_key(|p| p.col);
        }
        if let Some(lens) = self.lens_on(line) {
            pl.text.push(lens);
        }
        if let Some(note) = self.annotation_on(line) {
            pl.text.push(note);
        }
        if let Some(alert) = self.alert_on(line) {
            pl.text.push(alert);
        }
        if let Some(value) = self.eval_on(line) {
            pl.text.push(value);
        }

        // Inline AI suggestion ("ghost text") at the cursor, rendered grey.
        if let Some(ghost) = self.ghost.get_untracked() {
            let rope = self.inner.rope_text();
            let line_start = rope.offset_of_line(line);
            let line_end = rope.offset_of_line(line + 1);
            if ghost.offset >= line_start && ghost.offset < line_end {
                let (_, col) = rope.offset_to_line_col(ghost.offset);
                pl.text.push(PhantomText {
                    kind: PhantomTextKind::Completion,
                    col,
                    affinity: None,
                    text: ghost.text.clone(),
                    font_size: None,
                    fg: Some(ghost_color()),
                    bg: None,
                    under_line: None,
                });
                pl.text.sort_by_key(|p| p.col);
            }
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
