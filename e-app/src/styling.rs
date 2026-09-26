//! A Floem [`Styling`] that paints tree-sitter highlights and uses a
//! monospace font.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashSet;
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

/// Matching-bracket spans (line-local char ranges), one entry per line.
pub type BracketMarks = Rc<RefCell<Vec<Vec<(usize, usize)>>>>;

/// A diagnostic span within a single line (line-local char offsets).
#[derive(Clone, Copy)]
pub struct DiagSpan {
    pub start: usize,
    pub end: usize,
    pub error: bool,
}

/// Shared, mutable per-buffer diagnostic spans (one entry per line).
pub type DiagLines = Rc<RefCell<Vec<Vec<DiagSpan>>>>;

/// Shared, mutable set of 0-based lines carrying a debug breakpoint.
pub type BpMarks = Rc<RefCell<HashSet<usize>>>;

/// The 0-based line the debugger is currently stopped on, if any.
pub type StopLine = Rc<RefCell<Option<usize>>>;

pub struct SyntaxStyling {
    highlights: Highlights,
    diagnostics: DiagLines,
    git: GitMarks,
    find: FindMarks,
    brackets: BracketMarks,
    breakpoints: BpMarks,
    stop_line: StopLine,
    /// `(family list as a string, parsed)` — re-parsed only when the chosen
    /// font changes, since this is read for every line laid out.
    family: std::cell::RefCell<(String, Vec<FamilyOwned>)>,
    font_size: floem::reactive::RwSignal<usize>,
    tab_width: usize,
}

impl SyntaxStyling {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        highlights: Highlights,
        diagnostics: DiagLines,
        git: GitMarks,
        find: FindMarks,
        brackets: BracketMarks,
        breakpoints: BpMarks,
        stop_line: StopLine,
        font_size: floem::reactive::RwSignal<usize>,
        tab_width: usize,
    ) -> Self {
        Self {
            highlights,
            diagnostics,
            git,
            find,
            brackets,
            breakpoints,
            stop_line,
            family: std::cell::RefCell::new((String::new(), vec![FamilyOwned::Monospace])),
            font_size,
            tab_width,
        }
    }
}

/// Build per-line diagnostic spans from LSP diagnostics + the buffer text.
///
/// Columns are UTF-8 bytes: `e-lsp` converts the server's units on arrival, and
/// the text layout's hit-testing is byte-indexed, so bytes go straight through.
pub fn build_diag_lines(diags: &[Diagnostic], text: &str) -> Vec<Vec<DiagSpan>> {
    let line_lens: Vec<usize> = text
        .split_inclusive('\n')
        .map(|l| {
            let t = l
                .strip_suffix("\r\n")
                .or_else(|| l.strip_suffix('\n'))
                .unwrap_or(l);
            t.len()
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
        floem::reactive::SignalGet::get_untracked(&self.font_size)
    }

    fn font_family(&self, _edid: EditorId, _line: usize) -> Cow<'_, [FamilyOwned]> {
        let want = crate::theme::mono_family_untracked();
        let mut cache = self.family.borrow_mut();
        if cache.0 != want {
            *cache = (want.clone(), FamilyOwned::parse_list(&want).collect());
        }
        Cow::Owned(cache.1.clone())
    }

    fn tab_width(&self, _edid: EditorId, _line: usize) -> usize {
        self.tab_width
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
        // Debugger: highlight the line execution is stopped on (full-width
        // translucent band behind the text).
        if *self.stop_line.borrow() == Some(line) {
            for run in layout_line.text.layout_runs() {
                let height = (run.max_ascent + run.max_descent) as f64;
                let y = run.line_y as f64 - run.max_ascent as f64;
                layout_line.extra_style.push(LineExtraStyle {
                    x: 0.0,
                    y,
                    width: None, // fills the viewport width
                    height,
                    bg_color: Some(Color::from_rgba8(0xe5, 0xc0, 0x7b, 0x33)),
                    under_line: None,
                    wave_line: None,
                });
            }
        }

        let palette = theme::palette_untracked();
        // Matching-bracket highlight (subtle box behind the bracket chars).
        for (start, end) in self.brackets.borrow().get(line).into_iter().flatten() {
            let color = Color::from_rgba8(0x80, 0x90, 0xa0, 0x55);
            let styles = range_styles(&layout_line.text, *start, *end, Some(color), None);
            layout_line.extra_style.extend(styles);
        }

        // Find-match highlights (drawn first, behind text).
        for span in self.find.borrow().get(line).into_iter().flatten() {
            let color = if span.current {
                palette.find_current
            } else {
                palette.find_other
            };
            let styles = range_styles(&layout_line.text, span.start, span.end, Some(color), None);
            layout_line.extra_style.extend(styles);
        }

        // Debugger breakpoint marker: a red dot at the left margin.
        if self.breakpoints.borrow().contains(&line) {
            for run in layout_line.text.layout_runs() {
                let full = (run.max_ascent + run.max_descent) as f64;
                let size = full.min(9.0);
                let top = run.line_y as f64 - run.max_ascent as f64;
                let y = top + (full - size) / 2.0;
                layout_line.extra_style.push(LineExtraStyle {
                    x: 2.0,
                    y,
                    width: Some(size),
                    height: size,
                    bg_color: Some(palette.error),
                    under_line: None,
                    wave_line: None,
                });
            }
        }

        // Git change bar at the left edge of the line.
        if let Some(mark) = self.git.borrow().get(line).copied().flatten() {
            let color = match mark {
                LineMark::Added => palette.git_added,
                LineMark::Modified => palette.git_modified,
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
                palette.error
            } else {
                palette.warning
            };
            let styles = range_styles(&layout_line.text, span.start, span.end, None, Some(color));
            layout_line.extra_style.extend(styles);
        }
    }
}

/// Map a semantic token class to the theme's colour. `None` keeps the default
/// foreground.
fn color_for(kind: HighlightKind) -> Option<Color> {
    theme::syntax_color_untracked(kind)
}
