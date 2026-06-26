//! A Floem [`Styling`] that paints tree-sitter highlights and uses a
//! monospace font.

use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

use floem::peniko::Color;
use floem::text::{Attrs, AttrsList, FamilyOwned, TextLayout};
use floem::views::editor::id::EditorId;
use floem::views::editor::layout::{LineExtraStyle, TextLayoutLine};
use floem::views::editor::text::Styling;
use floem::views::editor::EditorStyle;
use lsp_types::{Diagnostic, DiagnosticSeverity};

use e_core::git::LineMark;
use e_core::syntax::{HighlightKind, LineSpan};

use crate::theme;

/// Shared, mutable per-buffer highlight data (one entry per line).
pub type Highlights = Rc<RefCell<Vec<Vec<LineSpan>>>>;

/// Shared, mutable per-buffer git change markers (one slot per line).
pub type GitMarks = Rc<RefCell<Vec<Option<LineMark>>>>;

/// A find-match span within a single line (line-local char offsets).
#[derive(Clone, Copy)]
pub struct FindSpan {
    pub start: usize,
    pub end: usize,
    pub current: bool,
}

/// Shared, mutable per-buffer find-match spans (one entry per line).
pub type FindMarks = Rc<RefCell<Vec<Vec<FindSpan>>>>;

/// A diagnostic span within a single line (line-local char offsets).
#[derive(Clone, Copy)]
pub struct DiagSpan {
    pub start: usize,
    pub end: usize,
    pub error: bool,
}

/// Shared, mutable per-buffer diagnostic spans (one entry per line).
pub type DiagLines = Rc<RefCell<Vec<Vec<DiagSpan>>>>;

pub struct SyntaxStyling {
    highlights: Highlights,
    diagnostics: DiagLines,
    git: GitMarks,
    find: FindMarks,
    family: Vec<FamilyOwned>,
    font_size: usize,
}

impl SyntaxStyling {
    pub fn new(highlights: Highlights, diagnostics: DiagLines, git: GitMarks, find: FindMarks) -> Self {
        Self {
            highlights,
            diagnostics,
            git,
            find,
            family: vec![FamilyOwned::Monospace],
            font_size: 14,
        }
    }
}

/// Build per-line diagnostic spans from LSP diagnostics + the buffer text.
pub fn build_diag_lines(diags: &[Diagnostic], text: &str) -> Vec<Vec<DiagSpan>> {
    let line_lens: Vec<usize> = text
        .split_inclusive('\n')
        .map(|l| {
            let t = l
                .strip_suffix("\r\n")
                .or_else(|| l.strip_suffix('\n'))
                .unwrap_or(l);
            t.chars().count()
        })
        .collect();
    let n = line_lens.len().max(1);
    let mut lines: Vec<Vec<DiagSpan>> = vec![Vec::new(); n];

    for d in diags {
        let error = !matches!(d.severity, Some(DiagnosticSeverity::WARNING));
        let sline = d.range.start.line as usize;
        let eline = (d.range.end.line as usize).min(n - 1);
        for line in sline..=eline {
            let len = line_lens.get(line).copied().unwrap_or(0);
            let start = if line == sline {
                d.range.start.character as usize
            } else {
                0
            }
            .min(len);
            let end = if line == eline {
                d.range.end.character as usize
            } else {
                len
            };
            let end = end.max(start + 1).min(len.max(start + 1));
            if line < lines.len() {
                lines[line].push(DiagSpan { start, end, error });
            }
        }
    }
    lines
}

/// Port of Lapce's `extra_styles_for_range`: turn a column range into pixel
/// rectangles, styled as a background / underline / wavy underline.
fn range_styles(
    text_layout: &TextLayout,
    start: usize,
    end: usize,
    bg_color: Option<Color>,
    wave_line: Option<Color>,
) -> Vec<LineExtraStyle> {
    let start_hit = text_layout.hit_position(start);
    let end_hit = text_layout.hit_position(end);
    text_layout
        .layout_runs()
        .enumerate()
        .filter_map(|(current_line, run)| {
            if current_line < start_hit.line || current_line > end_hit.line {
                return None;
            }
            let x = if current_line == start_hit.line {
                start_hit.point.x
            } else {
                run.glyphs.first().map(|g| g.x).unwrap_or(0.0) as f64
            };
            let end_x = if current_line == end_hit.line {
                end_hit.point.x
            } else {
                run.glyphs.last().map(|g| g.x + g.w).unwrap_or(0.0) as f64
            };
            let width = end_x - x;
            if width == 0.0 {
                return None;
            }
            let height = (run.max_ascent + run.max_descent) as f64;
            let y = run.line_y as f64 - run.max_ascent as f64;
            Some(LineExtraStyle {
                x,
                y,
                width: Some(width),
                height,
                bg_color,
                under_line: None,
                wave_line,
            })
        })
        .collect()
}

impl Styling for SyntaxStyling {
    fn id(&self) -> u64 {
        0
    }

    fn font_size(&self, _edid: EditorId, _line: usize) -> usize {
        self.font_size
    }

    fn font_family(&self, _edid: EditorId, _line: usize) -> Cow<'_, [FamilyOwned]> {
        Cow::Borrowed(&self.family)
    }

    fn apply_attr_styles(
        &self,
        _edid: EditorId,
        _style: &EditorStyle,
        line: usize,
        default: Attrs,
        attrs: &mut AttrsList,
    ) {
        let highlights = self.highlights.borrow();
        let Some(spans) = highlights.get(line) else {
            return;
        };
        for span in spans {
            if let Some(color) = color_for(span.kind) {
                attrs.add_span(span.start..span.end, default.clone().color(color));
            }
        }
    }

    fn apply_layout_styles(
        &self,
        _edid: EditorId,
        _style: &EditorStyle,
        line: usize,
        layout_line: &mut TextLayoutLine,
    ) {
        // Find-match highlights (drawn first, behind text).
        for span in self.find.borrow().get(line).into_iter().flatten() {
            let color = if span.current {
                Color::from_rgb8(0xc8, 0x8a, 0x3a)
            } else {
                Color::from_rgb8(0x5a, 0x53, 0x2a)
            };
            let styles = range_styles(&layout_line.text, span.start, span.end, Some(color), None);
            layout_line.extra_style.extend(styles);
        }

        // Git change bar at the left edge of the line.
        if let Some(mark) = self.git.borrow().get(line).copied().flatten() {
            let color = match mark {
                LineMark::Added => Color::from_rgb8(0x6a, 0xb0, 0x4a),
                LineMark::Modified => Color::from_rgb8(0x4a, 0x7c, 0xc0),
            };
            for run in layout_line.text.layout_runs() {
                let height = (run.max_ascent + run.max_descent) as f64;
                let y = run.line_y as f64 - run.max_ascent as f64;
                layout_line.extra_style.push(LineExtraStyle {
                    x: 0.0,
                    y,
                    width: Some(3.0),
                    height,
                    bg_color: Some(color),
                    under_line: None,
                    wave_line: None,
                });
            }
        }

        let diagnostics = self.diagnostics.borrow();
        let Some(spans) = diagnostics.get(line) else {
            return;
        };
        for span in spans {
            let color = if span.error {
                Color::from_rgb8(0xe0, 0x6c, 0x75)
            } else {
                Color::from_rgb8(0xe5, 0xc0, 0x7b)
            };
            let styles = range_styles(&layout_line.text, span.start, span.end, None, Some(color));
            layout_line.extra_style.extend(styles);
        }
    }
}

/// Map a semantic token class to a colour. `None` keeps the default foreground.
fn color_for(kind: HighlightKind) -> Option<Color> {
    use HighlightKind::*;
    let c = match kind {
        Keyword => Color::from_rgb8(0xc6, 0x78, 0xdd),     // purple
        Function | Constructor => Color::from_rgb8(0x61, 0xaf, 0xef), // blue
        Type => Color::from_rgb8(0xe5, 0xc0, 0x7b),        // yellow
        String => Color::from_rgb8(0x98, 0xc3, 0x79),      // green
        Number | Constant => Color::from_rgb8(0xd1, 0x9a, 0x66), // orange
        Comment => Color::from_rgb8(0x5c, 0x63, 0x70),     // gray
        Property => Color::from_rgb8(0xe0, 0x6c, 0x75),    // red
        Operator | Escape => Color::from_rgb8(0x56, 0xb6, 0xc2), // cyan
        Namespace => Color::from_rgb8(0xe5, 0xc0, 0x7b),
        Attribute => Color::from_rgb8(0x61, 0xaf, 0xef),
        Label | Tag => Color::from_rgb8(0xe0, 0x6c, 0x75),
        Punctuation => theme::FG_DIM,
        Variable => return None,
    };
    Some(c)
}
