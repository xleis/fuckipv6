//! Single-line native text input with GPUI IME support.
//!
//! GPUI sends composition, paste, selection and keyboard-editing operations
//! through `EntityInputHandler`. Keeping this adapter in one place prevents
//! page code from hand-editing UTF-8 strings or drawing a fake text cursor.

use std::ops::Range;

use fuckipv6_common::design_tokens::{DARK_THEME, DIMENSIONS, RADII, SPACING_PX, TYPOGRAPHY};
use gpui::{
    actions, div, fill, point, prelude::*, px, relative, rgba, size, App, Bounds, ClipboardItem,
    Context, CursorStyle, Element, ElementId, ElementInputHandler, Entity, EntityInputHandler,
    EventEmitter, FocusHandle, Focusable, GlobalElementId, KeyBinding, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, Render, ShapedLine,
    SharedString, Style, TextRun, UTF16Selection, UnderlineStyle, Window,
};
use unicode_segmentation::UnicodeSegmentation;

actions!(
    native_text_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        Paste,
        Cut,
        Copy,
        Escape,
    ]
);

/// Install the editor action bindings once per GPUI application.
pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some("TextInput")),
        KeyBinding::new("delete", Delete, Some("TextInput")),
        KeyBinding::new("left", Left, Some("TextInput")),
        KeyBinding::new("right", Right, Some("TextInput")),
        KeyBinding::new("shift-left", SelectLeft, Some("TextInput")),
        KeyBinding::new("shift-right", SelectRight, Some("TextInput")),
        KeyBinding::new("cmd-a", SelectAll, Some("TextInput")),
        KeyBinding::new("cmd-c", Copy, Some("TextInput")),
        KeyBinding::new("cmd-v", Paste, Some("TextInput")),
        KeyBinding::new("cmd-x", Cut, Some("TextInput")),
        KeyBinding::new("home", Home, Some("TextInput")),
        KeyBinding::new("end", End, Some("TextInput")),
        KeyBinding::new("escape", Escape, Some("TextInput")),
    ]);
}

/// Emitted after the text changes. Consumers can persist the value without
/// reaching into the input's private selection or composition state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextInputEvent {
    pub value: String,
}

struct TextInputState {
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
}

/// A native single-line input entity. The outer element owns the accessible
/// text-input role; the inner custom element registers GPUI's IME handler.
pub struct NativeTextInput {
    id: SharedString,
    aria_label: SharedString,
    placeholder: SharedString,
    content: SharedString,
    focus_handle: FocusHandle,
    state: TextInputState,
}

impl NativeTextInput {
    pub fn new(
        id: impl Into<SharedString>,
        aria_label: impl Into<SharedString>,
        placeholder: impl Into<SharedString>,
        value: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> Self {
        let content = value.into();
        let cursor = content.len();
        Self {
            id: id.into(),
            aria_label: aria_label.into(),
            placeholder: placeholder.into(),
            content,
            focus_handle: cx.focus_handle(),
            state: TextInputState {
                selected_range: cursor..cursor,
                selection_reversed: false,
                marked_range: None,
                last_layout: None,
                last_bounds: None,
                is_selecting: false,
            },
        }
    }

    pub fn value(&self) -> &str {
        &self.content
    }

    /// Replace the value from application state. This intentionally does not
    /// emit a change event, avoiding a parent/child update loop.
    pub fn set_value(&mut self, value: impl Into<SharedString>, cx: &mut Context<Self>) {
        let value = value.into();
        if self.content == value {
            return;
        }
        self.content = value;
        let cursor = self.content.len();
        self.state.selected_range = cursor..cursor;
        self.state.selection_reversed = false;
        self.state.marked_range = None;
        cx.notify();
    }

    fn emit_changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(TextInputEvent {
            value: self.content.to_string(),
        });
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.state.selected_range.start, cx);
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.state.selected_range.end), cx);
        } else {
            self.move_to(self.state.selected_range.end, cx);
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx);
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn escape(&mut self, _: &Escape, window: &mut Window, cx: &mut Context<Self>) {
        self.state.marked_range = None;
        window.blur(cx);
        cx.notify();
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.selected_range.is_empty() {
            let previous = self.previous_boundary(self.cursor_offset());
            if previous == self.cursor_offset() {
                window.play_system_bell();
                return;
            }
            self.select_to(previous, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.selected_range.is_empty() {
            let next = self.next_boundary(self.cursor_offset());
            if next == self.cursor_offset() {
                window.play_system_bell();
                return;
            }
            self.select_to(next, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text.replace(['\r', '\n'], " "), window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.state.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.state.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.state.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.state.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.state.selected_range = offset..offset;
        self.state.selection_reversed = false;
        self.state.marked_range = None;
        cx.notify();
    }

    fn cursor_offset(&self) -> usize {
        if self.state.selection_reversed {
            self.state.selected_range.start
        } else {
            self.state.selected_range.end
        }
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.state.selection_reversed {
            self.state.selected_range.start = offset;
        } else {
            self.state.selected_range.end = offset;
        }
        if self.state.selected_range.end < self.state.selected_range.start {
            self.state.selection_reversed = !self.state.selection_reversed;
            self.state.selected_range =
                self.state.selected_range.end..self.state.selected_range.start;
        }
        cx.notify();
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        let (Some(bounds), Some(line)) = (&self.state.last_bounds, &self.state.last_layout) else {
            return self.content.len();
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        line.closest_index_for_x(position.x - bounds.left())
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        self.state.is_selecting = true;
        let offset = self.index_for_mouse_position(event.position);
        if event.modifiers.shift {
            self.select_to(offset, cx);
        } else {
            self.move_to(offset, cx);
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.state.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(index, _)| (index < offset).then_some(index))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(index, _)| (index > offset).then_some(index))
            .unwrap_or(self.content.len())
    }
}

impl EventEmitter<TextInputEvent> for NativeTextInput {}

impl EntityInputHandler for NativeTextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.state.selected_range),
            reversed: self.state.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.state
            .marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.state.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.state.marked_range.clone())
            .unwrap_or_else(|| self.state.selected_range.clone());
        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        let cursor = range.start + new_text.len();
        self.state.selected_range = cursor..cursor;
        self.state.marked_range = None;
        self.emit_changed(cx);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.state.marked_range.clone())
            .unwrap_or_else(|| self.state.selected_range.clone());
        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.state.marked_range =
            (!new_text.is_empty()).then_some(range.start..range.start + new_text.len());
        self.state.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .map(|range| range.start + range.start..range.end + range.start)
            .unwrap_or_else(|| {
                let cursor = range.start + new_text.len();
                cursor..cursor
            });
        self.emit_changed(cx);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.state.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        Some(Bounds::from_corners(
            point(
                bounds.left() + layout.x_for_index(range.start),
                bounds.top(),
            ),
            point(
                bounds.left() + layout.x_for_index(range.end),
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let line_point = self.state.last_bounds?.localize(&point)?;
        let layout = self.state.last_layout.as_ref()?;
        let utf8_index = layout.index_for_x(point.x - line_point.x)?;
        Some(self.offset_to_utf16(utf8_index))
    }
}

impl Focusable for NativeTextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

struct TextElement {
    input: Entity<NativeTextInput>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.state.selected_range.clone();
        let cursor = input.cursor_offset();
        let style = window.text_style();

        let (display_text, text_color) = if content.is_empty() {
            (input.placeholder.clone(), DARK_THEME.colors.text_muted)
        } else {
            (content, DARK_THEME.colors.text)
        };
        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: rgba(text_color.rgba_hex()).into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked_range) = input.state.marked_range.as_ref() {
            [
                TextRun {
                    len: marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len() - marked_range.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &runs, None);
        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + line.x_for_index(cursor), bounds.top()),
                        size(
                            px((TYPOGRAPHY.sm.size_px as f32 / 7.0).max(1.5)),
                            bounds.bottom() - bounds.top(),
                        ),
                    ),
                    rgba(DARK_THEME.colors.accent.rgba_hex()),
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(
                            bounds.left() + line.x_for_index(selected_range.start),
                            bounds.top(),
                        ),
                        point(
                            bounds.left() + line.x_for_index(selected_range.end),
                            bounds.bottom(),
                        ),
                    ),
                    rgba(DARK_THEME.colors.accent_soft.with_alpha(179).rgba_hex()),
                )),
                None,
            )
        };
        PrepaintState {
            line: Some(line),
            cursor,
            selection,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection);
        }
        let line = prepaint.line.take().expect("text line is always shaped");
        line.paint(
            bounds.origin,
            window.line_height(),
            gpui::TextAlign::Left,
            None,
            window,
            cx,
        )
        .expect("text line paint");
        if focus_handle.is_focused(window) {
            if let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
        }
        self.input.update(cx, |input, _cx| {
            input.state.last_layout = Some(line);
            input.state.last_bounds = Some(bounds);
        });
    }
}

impl Render for NativeTextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = DARK_THEME.colors;
        let focused = self.focus_handle.is_focused(window);
        div()
            .id(self.id.clone())
            .key_context("TextInput")
            .role(gpui::Role::TextInput)
            .aria_label(self.aria_label.clone())
            .aria_value(self.content.clone())
            .aria_placeholder(self.placeholder.clone())
            .focusable()
            .tab_stop(true)
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::escape))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .flex()
            .items_center()
            .min_h(px(DIMENSIONS.control_md_px as f32))
            .px(px(SPACING_PX[3] as f32))
            .py(px(SPACING_PX[2] as f32))
            .overflow_hidden()
            .rounded(px(RADII.md_px as f32))
            .bg(rgba(colors.input_background.rgba_hex()))
            .border_1()
            .border_color(rgba(
                if focused {
                    colors.accent
                } else {
                    colors.border
                }
                .rgba_hex(),
            ))
            .text_size(px(TYPOGRAPHY.sm.size_px as f32))
            .line_height(px((TYPOGRAPHY.sm.size_px as f32
                * TYPOGRAPHY.sm.line_height)
                .round()))
            .text_color(rgba(colors.text.rgba_hex()))
            .child(TextElement { input: cx.entity() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    struct Harness {
        input: Entity<NativeTextInput>,
    }

    impl Render for Harness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().child(self.input.clone())
        }
    }

    fn bind(cx: &mut TestAppContext) {
        cx.update(bind_keys);
    }

    #[gpui::test]
    fn unicode_ime_composition_and_utf16_ranges_are_supported(cx: &mut TestAppContext) {
        bind(cx);
        let (harness, cx) = cx.add_window_view(|_, cx| {
            let input = cx.new(|cx| NativeTextInput::new("ime-test", "名称", "请输入", "", cx));
            Harness { input }
        });
        let input = cx.read_entity(&harness, |harness, _| harness.input.clone());
        cx.update(|window, cx| {
            window.focus(&input.read(cx).focus_handle.clone(), cx);
        });

        cx.simulate_input("日本");
        cx.read_entity(&input, |input, _| assert_eq!(input.value(), "日本"));

        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(None, "語", Some(1..1), window, cx);
            });
        });
        cx.read_entity(&input, |input, _| {
            assert_eq!(input.value(), "日本語");
            assert_eq!(input.state.marked_range, Some(6..9));
            assert_eq!(input.offset_to_utf16(6), 2);
        });
    }

    #[gpui::test]
    fn keyboard_selection_and_clipboard_use_native_actions(cx: &mut TestAppContext) {
        bind(cx);
        let (harness, cx) = cx.add_window_view(|_, cx| {
            let input =
                cx.new(|cx| NativeTextInput::new("clipboard-test", "名称", "请输入", "hello", cx));
            Harness { input }
        });
        let input = cx.read_entity(&harness, |harness, _| harness.input.clone());
        cx.update(|window, cx| {
            window.focus(&input.read(cx).focus_handle.clone(), cx);
        });

        cx.simulate_keystrokes("home shift-right shift-right");
        cx.simulate_keystrokes("cmd-c");
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("he".to_string())
        );
        cx.simulate_keystrokes("backspace");
        cx.read_entity(&input, |input, _| assert_eq!(input.value(), "llo"));
    }
}
