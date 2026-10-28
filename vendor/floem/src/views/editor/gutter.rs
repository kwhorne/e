use crate::{
    context::{EventCx, PaintCx},
    event::{Event, EventPropagation},
    id::ViewId,
    peniko::kurbo::{BezPath, Circle, Line, Point, Stroke},
    prop, prop_extractor,
    style::{Style, TextColor},
    style_class,
    text::{Attrs, AttrsList, TextLayout},
    view::View,
    views::Decorators,
    Renderer,
};
use floem_editor_core::{cursor::CursorMode, mode::Mode};
use floem_reactive::{RwSignal, SignalGet, SignalWith};
use peniko::color::palette;
use peniko::kurbo::Rect;
use peniko::Color;

use super::{CurrentLineColor, Editor};

prop!(pub LeftOfCenterPadding: f64 {} = 25.);
prop!(pub RightOfCenterPadding: f64 {} = 30.);
prop!(pub DimColor: Option<Color> {} = None);

prop_extractor! {
    GutterStyle {
        accent_color: TextColor,
        dim_color: DimColor,
        left_padding: LeftOfCenterPadding,
        right_padding: RightOfCenterPadding,
        current_line_color: CurrentLineColor,
    }
}
impl GutterStyle {
    fn gs_accent_color(&self) -> Color {
        self.accent_color().unwrap_or(palette::css::BLACK)
    }

    fn gs_dim_color(&self) -> Color {
        self.dim_color().unwrap_or(self.gs_accent_color())
    }
}

pub struct EditorGutterView {
    id: ViewId,
    editor: RwSignal<Editor>,
    full_width: f64,
    text_width: f64,
    gutter_style: GutterStyle,
}

style_class!(pub GutterClass);

/// Something the gutter shows beside a line, from [`Styling::gutter_marks`].
///
/// [`Styling::gutter_marks`]: super::text::Styling::gutter_marks
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GutterMark {
    /// The line changed: a bar along the gutter's edge, against the text.
    Bar(Color),
    /// Lines were removed just above this one: a wedge on that edge, at the
    /// line's top.
    Wedge(Color),
    /// A dot in the left lane, such as a breakpoint.
    Dot(Color),
    /// A tick in the lane beside the text, such as a passing test.
    Check(Color),
    /// A cross in that lane, such as a failing test.
    Cross(Color),
    /// A dash in that lane, such as a skipped test.
    Dash(Color),
}

/// Where in the gutter a click landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GutterLane {
    /// Left of the line numbers, where breakpoints go.
    Left,
    Numbers,
    /// Between the numbers and the text, where test results go.
    Right,
}

pub fn editor_gutter_view(editor: RwSignal<Editor>) -> EditorGutterView {
    let id = ViewId::new();

    EditorGutterView {
        id,
        editor,
        full_width: 0.0,
        text_width: 0.0,
        gutter_style: Default::default(),
    }
    .class(GutterClass)
}

impl View for EditorGutterView {
    fn id(&self) -> ViewId {
        self.id
    }

    fn debug_name(&self) -> std::borrow::Cow<'static, str> {
        "Editor Gutter View".into()
    }

    fn style_pass(&mut self, cx: &mut crate::context::StyleCx<'_>) {
        if self.gutter_style.read(cx) {
            cx.app_state_mut().request_paint(self.id());
        }
    }

    fn layout(&mut self, cx: &mut crate::context::LayoutCx) -> taffy::prelude::NodeId {
        cx.layout_node(self.id(), true, |_cx| {
            let (width, height) = (self.text_width, 10.0);
            let layout_node = self
                .id
                .taffy()
                .borrow_mut()
                .new_leaf(taffy::style::Style::DEFAULT)
                .unwrap();

            let style = Style::new()
                .width(self.gutter_style.left_padding() + width + self.gutter_style.right_padding())
                .height(height)
                .to_taffy_style();
            let _ = self.id.taffy().borrow_mut().set_style(layout_node, style);
            vec![layout_node]
        })
    }

    fn compute_layout(&mut self, _cx: &mut crate::context::ComputeLayoutCx) -> Option<Rect> {
        if let Some(width) = self.id.get_layout().map(|l| l.size.width as f64) {
            self.full_width = width;
        }

        let editor = self.editor.get_untracked();
        let edid = editor.id();
        let style = editor.style();
        // TODO: don't assume font family is constant for each line
        let family = style.font_family(edid, 0);
        let attrs = Attrs::new()
            .family(&family)
            .font_size(style.font_size(edid, 0) as f32);

        let attrs_list = AttrsList::new(attrs);

        let widest_text_width = self.compute_widest_text_width(&attrs_list);
        if (self.full_width
            - widest_text_width
            - self.gutter_style.left_padding()
            - self.gutter_style.right_padding())
        .abs()
            > 1e-2
        {
            self.text_width = widest_text_width;
            self.id.request_layout();
        }
        None
    }

    fn paint(&mut self, cx: &mut PaintCx) {
        let editor = self.editor.get_untracked();
        let edid = editor.id();

        let viewport = editor.viewport.get_untracked();
        let cursor = editor.cursor;
        let style = editor.style.get_untracked();

        let (offset, mode) = cursor.with_untracked(|c| (c.offset(), c.get_mode()));
        let last_line = editor.last_line();
        let current_line = editor.line_of_offset(offset);

        // TODO: don't assume font family is constant for each line
        let family = style.font_family(edid, 0);
        let accent_color = self.gutter_style.gs_accent_color();
        let dim_color = self.gutter_style.gs_dim_color();
        let attrs = Attrs::new()
            .family(&family)
            .color(dim_color)
            .font_size(style.font_size(edid, 0) as f32);
        let attrs_list = AttrsList::new(attrs.clone());
        let current_line_attrs_list = AttrsList::new(attrs.color(accent_color));
        let show_relative = editor.es.with_untracked(|es| es.modal())
            && editor.es.with_untracked(|es| es.modal_relative_line())
            && mode != Mode::Insert;

        self.text_width = self.compute_widest_text_width(&attrs_list);

        editor.screen_lines.with_untracked(|screen_lines| {
            for (line, y) in screen_lines.iter_lines_y() {
                // If it ends up outside the bounds of the file, stop trying to display line numbers
                if line > last_line {
                    break;
                }

                let line_height = f64::from(style.line_height(edid, line));

                let text = if show_relative {
                    if line == current_line {
                        line + 1
                    } else {
                        line.abs_diff(current_line)
                    }
                } else {
                    line + 1
                }
                .to_string();

                let mut text_layout = TextLayout::new();
                if line == current_line {
                    text_layout.set_text(&text, current_line_attrs_list.clone(), None);
                    if let Some(current_line_color) = self.gutter_style.current_line_color() {
                        cursor.with_untracked(|cursor| {
                            let highlight_current_line = match cursor.mode {
                                // TODO: check if shis should be 0 or 1
                                CursorMode::Normal(size) => size == 0,
                                CursorMode::Insert(ref sel) => sel.is_caret(),
                                CursorMode::Visual { .. } => false,
                            };

                            // Highlight the current line
                            if highlight_current_line {
                                for (_, end) in cursor.regions_iter() {
                                    // TODO: unsure if this is correct for wrapping lines
                                    let rvline = editor.rvline_of_offset(end, cursor.affinity);

                                    if let Some(info) = screen_lines.info(rvline) {
                                        let line_height =
                                            editor.line_height(info.vline_info.rvline.line);
                                        // the extra 1px is for a small line that appears between
                                        let rect = Rect::from_origin_size(
                                            (viewport.x0, info.vline_y - viewport.y0),
                                            (self.full_width + 1.1, f64::from(line_height)),
                                        );

                                        cx.fill(&rect, current_line_color, 0.0);
                                    }
                                }
                            }
                        })
                    }
                } else {
                    text_layout.set_text(&text, attrs_list.clone(), None);
                }
                let size = text_layout.size();
                let height = size.height;

                let pos = Point::new(
                    (self.full_width - (size.width) - self.gutter_style.right_padding()).max(0.0),
                    y + (line_height - height) / 2.0 - viewport.y0,
                );

                cx.draw_text(&text_layout, pos);

                let top = y - viewport.y0;
                for mark in style.gutter_marks(edid, line) {
                    self.paint_mark(cx, mark, top, line_height);
                }
            }
        });
    }

    fn event_before_children(&mut self, _cx: &mut EventCx, event: &Event) -> EventPropagation {
        let Event::PointerDown(pe) = event else {
            return EventPropagation::Continue;
        };
        if !pe.button.is_primary() {
            return EventPropagation::Continue;
        }
        let editor = self.editor.get_untracked();
        let edid = editor.id();
        let style = editor.style.get_untracked();
        let y = pe.pos.y + editor.viewport.get_untracked().y0;
        let line = editor.screen_lines.with_untracked(|lines| {
            lines.iter_lines_y().find_map(|(line, top)| {
                let height = f64::from(style.line_height(edid, line));
                (y >= top && y < top + height).then_some(line)
            })
        });
        let Some(line) = line.filter(|l| *l <= editor.last_line()) else {
            return EventPropagation::Continue;
        };
        let lane = if pe.pos.x < self.gutter_style.left_padding() {
            GutterLane::Left
        } else if pe.pos.x > self.full_width - self.gutter_style.right_padding() {
            GutterLane::Right
        } else {
            GutterLane::Numbers
        };
        if style.gutter_click(edid, line, lane) {
            EventPropagation::Stop
        } else {
            EventPropagation::Continue
        }
    }
}

impl EditorGutterView {
    /// Draw one mark for the line whose top is at `top`.
    fn paint_mark(&self, cx: &mut PaintCx, mark: GutterMark, top: f64, height: f64) {
        let mid_y = top + height / 2.0;
        // The change bar's right edge: a little short of the text, so a line
        // that starts at its first column doesn't run into it.
        let edge = self.full_width - 3.0;
        // The lane beside the text, between the numbers and the change bar.
        let right_x = self.full_width - self.gutter_style.right_padding() / 2.0 - 1.0;
        let left_x = self.gutter_style.left_padding() / 2.0;
        let size = (height * 0.32).clamp(3.5, 5.0);
        let pen = Stroke::new(1.6)
            .with_caps(peniko::kurbo::Cap::Round)
            .with_join(peniko::kurbo::Join::Round);
        match mark {
            GutterMark::Bar(color) => {
                let rect = Rect::new(edge - 3.0, top, edge, top + height);
                cx.fill(&rect, color, 0.0);
            }
            GutterMark::Wedge(color) => {
                let (x, s) = (edge, 4.0);
                let mut path = BezPath::new();
                path.move_to((x - s - 1.0, top - s));
                path.line_to((x, top));
                path.line_to((x - s - 1.0, top + s));
                path.close_path();
                cx.fill(&path, color, 0.0);
            }
            GutterMark::Dot(color) => {
                cx.fill(&Circle::new((left_x, mid_y), size), color, 0.0);
            }
            GutterMark::Check(color) => {
                let mut path = BezPath::new();
                path.move_to((right_x - size, mid_y));
                path.line_to((right_x - size * 0.3, mid_y + size * 0.7));
                path.line_to((right_x + size, mid_y - size * 0.8));
                cx.stroke(&path, color, &pen);
            }
            GutterMark::Cross(color) => {
                let s = size * 0.8;
                for (a, b) in [((-s, -s), (s, s)), ((-s, s), (s, -s))] {
                    let line =
                        Line::new((right_x + a.0, mid_y + a.1), (right_x + b.0, mid_y + b.1));
                    cx.stroke(&line, color, &pen);
                }
            }
            GutterMark::Dash(color) => {
                let line = Line::new((right_x - size * 0.7, mid_y), (right_x + size * 0.7, mid_y));
                cx.stroke(&line, color, &pen);
            }
        }
    }

    fn compute_widest_text_width(&mut self, attrs_list: &AttrsList) -> f64 {
        let last_line = self.editor.get_untracked().last_line() + 1;
        let mut text = TextLayout::new();
        text.set_text(&last_line.to_string(), attrs_list.clone(), None);
        text.size().width
    }
}
