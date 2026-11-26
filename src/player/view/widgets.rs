//! Small pieces the player's overlays share: chips, round icon buttons, text
//! buttons, the panel card and centred messages.

use gpui::{
    AnyElement, Context, Div, ElementId, FontWeight, MouseButton, MouseDownEvent, Rgba,
    SharedString, Stateful, Window, div, prelude::*, px, rgb, rgba, svg,
};

use super::{BAR_HEIGHT, BORDER, PANEL_WIDTH, SURFACE, TEXT};
use crate::player::Player;
use crate::ui::icons::Icon;

/// What lies under the mouse on a button or a row.
pub(super) const HOVER: u32 = 0xffffff1f;
/// The white behind a lit button or tab, at its full strength (alpha).
const LIGHT_ALPHA: f32 = 0x26 as f32;

/// How much a text button stands out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Emphasis {
    /// The thing most likely wanted: light on dark.
    Primary,
    Quiet,
}

pub(super) fn chip() -> Div {
    div()
        .rounded_full()
        .bg(rgba(SURFACE))
        .border_1()
        .border_color(rgba(BORDER))
        .text_color(rgb(TEXT))
}

/// The background of a button or tab lit by `light` (0 to 1).
pub(super) fn light(light: f32) -> Rgba {
    rgba(0xffffff00 | (LIGHT_ALPHA * light).round() as u32)
}

/// `button`, lit by `active` (0 to 1): a button whose panel is open lights
/// up, following that panel's fade.
pub(super) fn lit(button: Stateful<Div>, active: f32) -> Stateful<Div> {
    button.when(active > 0.0, |button| button.bg(light(active)))
}

impl Player {
    /// A round icon button; a changed `glyph` dissolves into the new one.
    pub(super) fn icon_button(
        &self,
        id: impl Into<ElementId>,
        glyph: Icon,
        size: f32,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        let id = id.into();
        let glyph = self
            .fades
            .swaps
            .draw(format!("icon-{id:?}"), glyph.path(), move |path| {
                svg().path(path).size(px(size)).text_color(rgb(TEXT))
            });
        div()
            .id(id)
            .size(px(40.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .cursor_pointer()
            .hover(|s| s.bg(rgba(HOVER)))
            .child(glyph)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    action(this, window, cx);
                    this.poke(cx);
                }),
            )
    }

    /// The card that panels sit in, just above the control bar.
    pub(super) fn panel_frame(id: &'static str) -> Stateful<Div> {
        div()
            .id(id)
            .absolute()
            .bottom(px(BAR_HEIGHT + 10.0))
            .right_0()
            .w(px(PANEL_WIDTH))
            .max_w_full()
            .p_3()
            .rounded_2xl()
            .bg(rgba(SURFACE))
            .border_1()
            .border_color(rgba(BORDER))
            .text_color(rgb(TEXT))
            // Clicks inside the panel must not close it. No hover tracking: an open
            // panel keeps the controls up, and a hover flag would stick when it closes.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
    }

    pub(super) fn text_button(
        id: &'static str,
        label: &'static str,
        emphasis: Emphasis,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        let primary = emphasis == Emphasis::Primary;
        div()
            .id(id)
            .px_4()
            .py_1p5()
            .rounded_full()
            .text_sm()
            .font_weight(FontWeight::SEMIBOLD)
            .cursor_pointer()
            .bg(rgba(if primary { 0xf4f5f7ff } else { HOVER }))
            .text_color(rgb(if primary { 0x101218 } else { TEXT }))
            .hover(|s| s.opacity(0.85))
            .child(label)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    action(this, cx);
                }),
            )
            .into_any_element()
    }

    /// A message in a chip, centred across the window `top` pixels down.
    pub(super) fn centered_message(top: f32, text: SharedString, shown: f32) -> AnyElement {
        div()
            .absolute()
            .opacity(shown)
            .top(px(top))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(chip().px_4().py_2().text_sm().child(text))
            .into_any_element()
    }
}
