use std::any::Any;
use std::mem::swap;

use floem_reactive::create_effect;
use floem_renderer::{
    text::{Attrs, AttrsList, AttrsOwned, Cursor, TextLayout},
    Renderer,
};
use peniko::{
    color::palette,
    kurbo::{Point, Rect},
    Color,
};
use smallvec::{smallvec, SmallVec};
use taffy::tree::NodeId;
use winit::keyboard::Key;

use super::TextCommand;
use crate::{
    context::{PaintCx, UpdateCx},
    event::{Event, EventPropagation},
    id::ViewId,
    keyboard::KeyEvent,
    style::{Style, TextOverflow},
    unit::PxPct,
    view::View,
    Clipboard, IntoView,
};

/// Default translucent highlight for selected rich text.
const DEFAULT_SELECTION_COLOR: Color = Color::from_rgba8(0x3d, 0x6e, 0xb5, 0x66);

#[derive(Debug, Clone, Copy)]
enum SelectionState {
    None,
    Ready(Point),
    Selecting(Point, Point),
    Selected(Point, Point),
}

/// Reconstruct the plain text of a layout (logical lines joined by `\n`) so a
/// selection can be copied and hit offsets mapped back to bytes.
fn layout_text(layout: &TextLayout) -> String {
    layout
        .lines()
        .iter()
        .map(|l| l.text())
        .collect::<Vec<_>>()
        .join("\n")
}

pub struct RichText {
    id: ViewId,
    text_layout: TextLayout,
    text_node: Option<NodeId>,
    text_overflow: TextOverflow,
    available_width: Option<f32>,
    available_text_layout: Option<TextLayout>,
    // Text selection (opt-in via `RichText::selectable`).
    text: String,
    selectable: bool,
    selection_state: SelectionState,
    selection_range: Option<(Cursor, Cursor)>,
    selection_color: Color,
    // Fired on a plain click (no drag) with the byte offset in `text` that was
    // clicked — used for click-to-open file links in terminal/agent output.
    on_click_offset: Option<Box<dyn Fn(usize)>>,
}

impl RichText {
    /// Allow the user to select and copy this text with the pointer + `Cmd/Ctrl+C`.
    pub fn selectable(mut self) -> Self {
        self.selectable = true;
        self
    }

    /// Override the selection highlight color.
    pub fn selection_color(mut self, color: Color) -> Self {
        self.selection_color = color;
        self
    }

    /// Call `f` with the clicked byte offset (into the rendered text) on a plain
    /// click that isn't a drag-selection.
    pub fn on_click_offset(mut self, f: impl Fn(usize) + 'static) -> Self {
        self.on_click_offset = Some(Box::new(f));
        self
    }

    fn cursor_to_offset(&self, c: Cursor) -> usize {
        let ranges = self.effective_text_layout().lines_range();
        if c.line < ranges.len() {
            (ranges[c.line].start + c.index).min(self.text.len())
        } else {
            self.text.len()
        }
    }

    fn effective_text_layout(&self) -> &TextLayout {
        self.available_text_layout
            .as_ref()
            .unwrap_or(&self.text_layout)
    }

    fn get_hit_point(&self, point: Point) -> Option<Cursor> {
        let text_node = self.text_node?;
        let location = self
            .id
            .taffy()
            .borrow()
            .layout(text_node)
            .map_or(taffy::Layout::new().location, |layout| layout.location);
        self.effective_text_layout()
            .hit(point.x as f32 - location.x, point.y as f32 - location.y)
    }

    fn set_selection_range(&mut self) {
        match self.selection_state {
            SelectionState::None => {
                self.selection_range = None;
            }
            SelectionState::Selecting(start, end) | SelectionState::Selected(start, end) => {
                let Some(mut start_cursor) = self.get_hit_point(start) else {
                    return;
                };
                if let Some(mut end_cursor) = self.get_hit_point(end) {
                    if start_cursor.line > end_cursor.line
                        || (start_cursor.line == end_cursor.line
                            && start_cursor.index > end_cursor.index)
                    {
                        swap(&mut start_cursor, &mut end_cursor);
                    }
                    self.selection_range = Some((start_cursor, end_cursor));
                }
            }
            SelectionState::Ready(_) => {}
        }
    }

    fn copy_selection(&self) {
        let Some((start_c, end_c)) = &self.selection_range else {
            return;
        };
        let ranges = self.effective_text_layout().lines_range();
        if start_c.line >= ranges.len() || end_c.line >= ranges.len() {
            return;
        }
        let start_idx = ranges[start_c.line].start + start_c.index;
        let end_idx = ranges[end_c.line].start + end_c.index;
        if start_idx <= end_idx && end_idx <= self.text.len() {
            let _ = Clipboard::set_contents(self.text[start_idx..end_idx].to_string());
        }
    }

    fn handle_key_down(&mut self, event: &KeyEvent) -> bool {
        if event.modifiers.is_empty() {
            return false;
        }
        if let Key::Character(ref ch) = event.key.logical_key {
            if let TextCommand::Copy = (event, ch).into() {
                self.copy_selection();
                return true;
            }
        }
        false
    }

    fn paint_selection(&self, text_layout: &TextLayout, cx: &mut PaintCx) {
        let Some((start_c, end_c)) = &self.selection_range else {
            return;
        };
        let location = self
            .id
            .taffy()
            .borrow()
            .layout(self.text_node.unwrap())
            .cloned()
            .unwrap_or_default()
            .location;
        for run in text_layout.layout_runs() {
            if let Some((mut start_x, width)) = run.highlight(*start_c, *end_c) {
                start_x += location.x;
                let end_x = width + start_x;
                let start_y = location.y as f64 + run.line_top as f64;
                let end_y = start_y + run.line_height as f64;
                let rect =
                    Rect::new(start_x.into(), start_y, end_x.into(), end_y).to_rounded_rect(2.0);
                cx.fill(&rect, &self.selection_color, 0.0);
            }
        }
    }
}

pub fn rich_text(text_layout: impl Fn() -> TextLayout + 'static) -> RichText {
    let id = ViewId::new();
    let layout = text_layout();
    let text = layout_text(&layout);
    create_effect(move |_| {
        let new_text_layout = text_layout();
        id.update_state(new_text_layout);
    });
    RichText {
        id,
        text_layout: layout,
        text_node: None,
        text_overflow: TextOverflow::Wrap,
        available_width: None,
        available_text_layout: None,
        text,
        selectable: false,
        selection_state: SelectionState::None,
        selection_range: None,
        selection_color: DEFAULT_SELECTION_COLOR,
        on_click_offset: None,
    }
}

impl View for RichText {
    fn id(&self) -> ViewId {
        self.id
    }

    fn debug_name(&self) -> std::borrow::Cow<'static, str> {
        format!(
            "RichText: {:?}",
            self.text_layout
                .lines()
                .iter()
                .map(|text| text.text())
                .collect::<String>()
        )
        .into()
    }

    fn update(&mut self, _cx: &mut UpdateCx, state: Box<dyn Any>) {
        if let Ok(state) = state.downcast() {
            self.text_layout = *state;
            self.text = layout_text(&self.text_layout);
            self.available_width = None;
            self.available_text_layout = None;
            self.id.request_layout();
        }
    }

    fn event_before_children(
        &mut self,
        _cx: &mut crate::context::EventCx,
        event: &Event,
    ) -> EventPropagation {
        if !self.selectable && self.on_click_offset.is_none() {
            return EventPropagation::Continue;
        }
        match event {
            Event::PointerDown(pe) => {
                self.selection_range = None;
                self.selection_state = SelectionState::Ready(pe.pos);
                self.id.request_layout();
            }
            Event::PointerMove(pme) => {
                let (SelectionState::Selecting(start, _) | SelectionState::Ready(start)) =
                    self.selection_state
                else {
                    return EventPropagation::Continue;
                };
                // Ignore tiny moves so a click doesn't eat pointer events.
                if start.distance(pme.pos).abs() > 2. {
                    self.selection_state = SelectionState::Selecting(start, pme.pos);
                    self.id.request_active();
                    self.id.request_focus();
                    self.id.request_layout();
                }
            }
            Event::PointerUp(pe) => {
                let was_click = matches!(self.selection_state, SelectionState::Ready(_));
                if let SelectionState::Selecting(start, end) = self.selection_state {
                    self.selection_state = SelectionState::Selected(start, end);
                } else {
                    self.selection_state = SelectionState::None;
                }
                self.id.clear_active();
                self.id.request_layout();
                // A click without a drag — report the byte offset that was hit.
                if was_click {
                    if let Some(cb) = self.on_click_offset.as_ref() {
                        if let Some(cursor) = self.get_hit_point(pe.pos) {
                            cb(self.cursor_to_offset(cursor));
                        }
                    }
                }
            }
            Event::KeyDown(ke) => {
                if self.handle_key_down(ke) {
                    return EventPropagation::Stop;
                }
            }
            _ => {}
        }
        EventPropagation::Continue
    }

    fn layout(&mut self, cx: &mut crate::context::LayoutCx) -> taffy::tree::NodeId {
        cx.layout_node(self.id(), true, |_cx| {
            let size = self.text_layout.size();
            let width = size.width as f32;
            let mut height = size.height as f32;

            if let Some(t) = self.available_text_layout.as_ref() {
                height = height.max(t.size().height as f32);
            }

            if self.text_node.is_none() {
                self.text_node = Some(
                    self.id
                        .taffy()
                        .borrow_mut()
                        .new_leaf(taffy::style::Style::DEFAULT)
                        .unwrap(),
                );
            }
            let text_node = self.text_node.unwrap();

            let style = Style::new().width(width).height(height).to_taffy_style();
            let _ = self.id.taffy().borrow_mut().set_style(text_node, style);
            vec![text_node]
        })
    }

    fn compute_layout(&mut self, _cx: &mut crate::context::ComputeLayoutCx) -> Option<Rect> {
        let layout = self.id.get_layout().unwrap_or_default();
        let view_state = self.id.state();
        let (padding_left, padding_right) = {
            let view_state = view_state.borrow();
            let style = view_state.combined_style.builtin();
            let padding_left = match style.padding_left() {
                PxPct::Px(padding) => padding as f32,
                PxPct::Pct(pct) => pct as f32 * layout.size.width,
            };
            let padding_right = match style.padding_right() {
                PxPct::Px(padding) => padding as f32,
                PxPct::Pct(pct) => pct as f32 * layout.size.width,
            };
            self.text_overflow = style.text_overflow();
            (padding_left, padding_right)
        };

        let padding = padding_left + padding_right;
        let width = self.text_layout.size().width as f32;
        let available_width = layout.size.width - padding;
        if self.text_overflow == TextOverflow::Wrap {
            if width > available_width {
                if self.available_width != Some(available_width) {
                    let mut text_layout = self.text_layout.clone();
                    text_layout.set_size(available_width, f32::MAX);
                    self.available_text_layout = Some(text_layout);
                    self.available_width = Some(available_width);
                    self.id.request_layout();
                }
            } else {
                if self.available_text_layout.is_some() {
                    self.id.request_layout();
                }
                self.available_text_layout = None;
                self.available_width = None;
            }
        }

        self.set_selection_range();
        None
    }

    fn paint(&mut self, cx: &mut crate::context::PaintCx) {
        let text_node = self.text_node.unwrap();
        let location = self
            .id
            .taffy()
            .borrow_mut()
            .layout(text_node)
            .cloned()
            .unwrap_or_default()
            .location;
        let point = Point::new(location.x as f64, location.y as f64);
        let text_layout = self
            .available_text_layout
            .as_ref()
            .unwrap_or(&self.text_layout);
        cx.draw_text(text_layout, point);
        if self.selectable && cx.app_state.is_focused(&self.id) {
            self.paint_selection(text_layout, cx);
        }
    }
}

#[derive(Clone, Debug)]
pub struct RichSpan<'a> {
    text: &'a str,
    attrs: Attrs<'a>,
}
#[allow(clippy::wrong_self_convention)]
impl<'a> RichSpan<'a> {
    fn to_owned(self) -> RichSpanOwned {
        let len = self.text.len();
        RichSpanOwned {
            text: self.text.to_string(),
            spans: smallvec::smallvec![(0..len, AttrsOwned::new(self.attrs))],
        }
    }
    pub fn color(mut self, color: Color) -> Self {
        self.attrs = self.attrs.color(color);
        self
    }

    pub fn family(mut self, family: &'a [floem_renderer::text::FamilyOwned]) -> RichSpan<'a> {
        self.attrs = self.attrs.family(family);
        self
    }

    pub fn stretch(mut self, stretch: floem_renderer::text::Stretch) -> RichSpan<'a> {
        self.attrs = self.attrs.stretch(stretch);
        self
    }

    pub fn text_style(mut self, style: floem_renderer::text::Style) -> RichSpan<'a> {
        self.attrs = self.attrs.style(style);
        self
    }

    pub fn weight(mut self, weight: floem_renderer::text::Weight) -> RichSpan<'a> {
        self.attrs = self.attrs.weight(weight);
        self
    }

    pub fn line_height(
        mut self,
        line_height: floem_renderer::text::LineHeightValue,
    ) -> RichSpan<'a> {
        self.attrs = self.attrs.line_height(line_height);
        self
    }

    pub fn font_size(mut self, font_size: f32) -> RichSpan<'a> {
        self.attrs = self.attrs.font_size(font_size);
        self
    }

    pub fn raw_weight(mut self, weight: u16) -> RichSpan<'a> {
        self.attrs = self.attrs.raw_weight(weight);
        self
    }
}
#[derive(Clone, Debug)]
pub struct RichSpanOwned {
    text: String,
    spans: SmallVec<[(std::ops::Range<usize>, AttrsOwned); 3]>,
}
impl IntoView for RichSpanOwned {
    type V = RichText;

    fn into_view(self) -> Self::V {
        let mut layout = TextLayout::new();
        let mut attrs_list = AttrsList::new(Attrs::new().color(palette::css::BLACK));
        for span in self.spans {
            attrs_list.add_span(span.0, span.1.as_attrs());
        }

        layout.set_text(&self.text, attrs_list, None);
        rich_text(move || layout.clone())
    }
}
impl IntoView for RichSpan<'_> {
    type V = RichText;

    fn into_view(self) -> Self::V {
        self.to_owned().into_view()
    }
}
impl<'a, S> std::ops::Add<S> for RichSpan<'a>
where
    RichSpan<'a>: From<S>,
{
    type Output = RichSpanOwned;

    fn add(self, rhs: S) -> Self::Output {
        let self_len = self.text.len();
        let rhs: RichSpan = rhs.into();
        let rhs_len = rhs.text.len();
        RichSpanOwned {
            text: self.text.to_string() + rhs.text,
            spans: smallvec![
                (0..self_len, AttrsOwned::new(self.attrs)),
                (self_len..self_len + rhs_len, AttrsOwned::new(rhs.attrs)),
            ],
        }
    }
}
impl<'a> std::ops::Add<&'a str> for RichSpan<'a> {
    type Output = RichSpanOwned;

    fn add(self, rhs: &'a str) -> Self::Output {
        let self_len = self.text.len();
        let rhs_len = rhs.len();
        RichSpanOwned {
            text: self.text.to_string() + rhs,
            spans: smallvec![
                (0..self_len, AttrsOwned::new(self.attrs)),
                (
                    self_len..self_len + rhs_len,
                    AttrsOwned::new(Attrs::new().color(palette::css::BLACK))
                ),
            ],
        }
    }
}
impl std::ops::Add<String> for RichSpan<'_> {
    type Output = RichSpanOwned;

    fn add(self, rhs: String) -> Self::Output {
        let self_len = self.text.len();
        let rhs_len = rhs.len();
        RichSpanOwned {
            text: self.text.to_string() + &rhs,
            spans: smallvec![
                (0..self_len, AttrsOwned::new(self.attrs)),
                (
                    self_len..self_len + rhs_len,
                    AttrsOwned::new(Attrs::new().color(palette::css::BLACK))
                ),
            ],
        }
    }
}
impl<'a, S> std::ops::Add<S> for RichSpanOwned
where
    RichSpan<'a>: From<S>,
{
    type Output = Self;

    fn add(mut self, rhs: S) -> Self::Output {
        let rhs: RichSpan = rhs.into();
        let self_len = self.text.len();
        let new_text = self.text + rhs.text;
        self.spans
            .push((self_len..new_text.len(), AttrsOwned::new(rhs.attrs)));
        Self {
            text: new_text,
            spans: self.spans,
        }
    }
}
impl std::ops::Add<&str> for RichSpanOwned {
    type Output = RichSpanOwned;

    fn add(mut self, rhs: &str) -> Self::Output {
        let self_len = self.text.len();
        let new_text = self.text + rhs;
        self.spans.push((
            self_len..new_text.len(),
            AttrsOwned::new(Attrs::new().color(palette::css::BLACK)),
        ));
        Self {
            text: new_text,
            spans: self.spans,
        }
    }
}
impl std::ops::Add<String> for RichSpanOwned {
    type Output = RichSpanOwned;

    fn add(mut self, rhs: String) -> Self::Output {
        let self_len = self.text.len();
        let new_text = self.text + &rhs;
        self.spans.push((
            self_len..new_text.len(),
            AttrsOwned::new(Attrs::new().color(palette::css::BLACK)),
        ));
        Self {
            text: new_text,
            spans: self.spans,
        }
    }
}
impl std::ops::Add for RichSpanOwned {
    type Output = Self;

    fn add(mut self, rhs: Self) -> Self::Output {
        let self_len = self.text.len();
        self.spans.extend(
            rhs.spans
                .into_iter()
                .map(|span| ((span.0.start + self_len)..(span.0.end + self_len), span.1)),
        );
        Self {
            text: self.text + &rhs.text,
            spans: self.spans,
        }
    }
}

pub trait RichTextExt<'a>
where
    Self: Sized,
    RichSpan<'a>: From<Self>,
{
    fn color(self, color: Color) -> RichSpan<'a> {
        let span: RichSpan = self.into();
        span.color(color)
    }
    fn red(self) -> RichSpan<'a> {
        self.color(palette::css::RED)
    }
    fn blue(self) -> RichSpan<'a> {
        self.color(palette::css::BLUE)
    }

    fn green(self) -> RichSpan<'a> {
        self.color(palette::css::GREEN)
    }

    fn yellow(self) -> RichSpan<'a> {
        self.color(palette::css::YELLOW)
    }

    fn black(self) -> RichSpan<'a> {
        self.color(palette::css::BLACK)
    }

    fn white(self) -> RichSpan<'a> {
        self.color(palette::css::WHITE)
    }

    fn gray(self) -> RichSpan<'a> {
        self.color(palette::css::GRAY)
    }

    fn cyan(self) -> RichSpan<'a> {
        self.color(palette::css::CYAN)
    }

    fn magenta(self) -> RichSpan<'a> {
        self.color(palette::css::MAGENTA)
    }

    fn orange(self) -> RichSpan<'a> {
        self.color(palette::css::ORANGE)
    }

    fn purple(self) -> RichSpan<'a> {
        self.color(palette::css::PURPLE)
    }

    fn pink(self) -> RichSpan<'a> {
        self.color(palette::css::PINK)
    }

    fn family(self, family: &'a [crate::text::FamilyOwned]) -> RichSpan<'a> {
        let span: RichSpan = self.into();
        span.family(family)
    }
    fn stretch(self, stretch: crate::text::Stretch) -> RichSpan<'a> {
        let span: RichSpan = self.into();
        span.stretch(stretch)
    }
    fn text_style(self, style: crate::text::Style) -> RichSpan<'a> {
        let span: RichSpan = self.into();
        span.text_style(style)
    }
    fn italic(self) -> RichSpan<'a> {
        self.text_style(crate::text::Style::Italic)
    }
    fn oblique(self) -> RichSpan<'a> {
        self.text_style(crate::text::Style::Oblique)
    }

    fn weight(self, weight: crate::text::Weight) -> RichSpan<'a> {
        let span: RichSpan = self.into();
        span.weight(weight)
    }
    fn thin(self) -> RichSpan<'a> {
        self.weight(crate::text::Weight::THIN)
    }
    fn extra_light(self) -> RichSpan<'a> {
        self.weight(crate::text::Weight::EXTRA_LIGHT)
    }
    fn light(self) -> RichSpan<'a> {
        self.weight(crate::text::Weight::LIGHT)
    }
    fn medium(self) -> RichSpan<'a> {
        self.weight(crate::text::Weight::MEDIUM)
    }
    fn semibold(self) -> RichSpan<'a> {
        self.weight(crate::text::Weight::SEMIBOLD)
    }
    fn bold(self) -> RichSpan<'a> {
        self.weight(crate::text::Weight::BOLD)
    }
    fn extra_bold(self) -> RichSpan<'a> {
        self.weight(crate::text::Weight::EXTRA_BOLD)
    }

    fn raw_weight(self, weight: u16) -> RichSpan<'a> {
        let span: RichSpan = self.into();
        span.raw_weight(weight)
    }
    fn font_size(self, font_size: f32) -> RichSpan<'a> {
        let span: RichSpan = self.into();
        span.font_size(font_size)
    }

    fn line_height(self, line_height: crate::text::LineHeightValue) -> RichSpan<'a> {
        let span: RichSpan = self.into();
        span.line_height(line_height)
    }
}

impl<'a, S> RichTextExt<'a> for S
where
    S: AsRef<str>,
    RichSpan<'a>: From<S>,
{
}
impl<'a, S: AsRef<str> + 'a> From<&'a S> for RichSpan<'a> {
    fn from(value: &'a S) -> Self {
        RichSpan {
            text: value.as_ref(),
            attrs: Attrs::new().color(palette::css::BLACK),
        }
    }
}
impl<'a> RichTextExt<'a> for RichSpan<'a> {}
