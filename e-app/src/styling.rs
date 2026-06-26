//! A Floem [`Styling`] that paints tree-sitter highlights and uses a
//! monospace font.

use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

use floem::peniko::Color;
use floem::text::{Attrs, AttrsList, FamilyOwned};
use floem::views::editor::id::EditorId;
use floem::views::editor::text::Styling;
use floem::views::editor::EditorStyle;

use e_core::syntax::{HighlightKind, LineSpan};

use crate::theme;

/// Shared, mutable per-buffer highlight data (one entry per line).
pub type Highlights = Rc<RefCell<Vec<Vec<LineSpan>>>>;

pub struct SyntaxStyling {
    highlights: Highlights,
    family: Vec<FamilyOwned>,
    font_size: usize,
}

impl SyntaxStyling {
    pub fn new(highlights: Highlights) -> Self {
        Self {
            highlights,
            family: vec![FamilyOwned::Monospace],
            font_size: 14,
        }
    }
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
