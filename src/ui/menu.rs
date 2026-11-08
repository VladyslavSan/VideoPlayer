//! Dropdown menus: a button showing the current choice, and a list of
//! choices that pops up over everything else when it's clicked.

use gpui::{
    AnyElement, App, Corner, Div, ElementId, FontWeight, MouseButton, MouseDownEvent, SharedString,
    Stateful, Window, anchored, deferred, div, point, prelude::*, px, rgb, rgba,
};

use super::icons::{Icon, icon};

const TEXT: u32 = 0xf4f5f7;
const ACCENT: u32 = 0x4c8dff;
const SURFACE: u32 = 0x1c1f27;
const BORDER: u32 = 0xffffff1f;
/// Gap between the button and its menu.
const GAP: f32 = 4.0;
/// The trigger's height, which `popup` keeps clear when it places the menu.
const TRIGGER_HEIGHT: f32 = 32.0;

/// The button that opens a menu: the current choice and a chevron.
pub fn trigger(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    open: bool,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex()
        .flex_none()
        .items_center()
        .gap_1p5()
        .h(px(TRIGGER_HEIGHT))
        .pl_3()
        .pr_2()
        .rounded_md()
        .text_sm()
        .text_color(rgb(TEXT))
        .bg(rgba(if open { 0xffffff26 } else { 0xffffff14 }))
        .hover(|s| s.bg(rgba(0xffffff26)))
        .cursor_pointer()
        .child(label.into())
        .child(icon(Icon::ChevronDown).size(px(14.0)).text_color(rgb(TEXT)))
}

/// One choice in a menu, with a check mark if it's the current one.
/// Attach `on_click` to act on it.
pub fn item(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    selected: bool,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex()
        .items_center()
        .gap_2()
        .pl_2()
        .pr_4()
        .py_1p5()
        .rounded_md()
        .text_sm()
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|s| s.bg(rgba(0xffffff1a)))
        .when(selected, |d| d.font_weight(FontWeight::MEDIUM))
        .child(div().size(px(16.0)).flex_none().when(selected, |d| {
            d.child(icon(Icon::Check).size(px(16.0)).text_color(rgb(ACCENT)))
        }))
        .child(label.into())
}

/// The open menu, drawn on top of everything, below (or, with `up`, above)
/// the trigger. Place it in a `relative()` element that wraps the trigger.
/// If the menu doesn't fit on that side of the trigger it opens on the
/// other. Clicking anywhere outside it calls `dismiss`; the click goes no
/// further, so it doesn't also act on whatever is underneath.
pub fn popup(
    up: bool,
    items: Vec<AnyElement>,
    window: &Window,
    dismiss: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let viewport = window.viewport_size();
    let dismiss = std::rc::Rc::new(dismiss);
    let close = || {
        let dismiss = dismiss.clone();
        move |event: &MouseDownEvent, window: &mut Window, cx: &mut App| {
            cx.stop_propagation();
            dismiss(event, window, cx);
        }
    };
    let backdrop = anchored().position(point(px(0.0), px(0.0))).child(
        div()
            .id("menu-backdrop")
            .occlude()
            .w(viewport.width)
            .h(viewport.height)
            .on_mouse_down(MouseButton::Left, close())
            .on_mouse_down(MouseButton::Right, close()),
    );
    let card = div()
        .id("menu")
        .occlude()
        .flex()
        .flex_col()
        .gap_0p5()
        .p_1()
        .rounded_lg()
        .bg(rgb(SURFACE))
        .border_1()
        .border_color(rgba(BORDER))
        .text_color(rgb(TEXT))
        .shadow_lg()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .children(items);
    // Anchored at the trigger's top-left corner. GPUI flips an anchored
    // element to the other side of that point when it doesn't fit, so the
    // card is padded (with empty space that doesn't take clicks) by the
    // trigger's height above it and a gap below: hanging down from the
    // point it clears the trigger, and standing on it, it ends a gap above.
    let menu = anchored()
        .anchor(if up {
            Corner::BottomLeft
        } else {
            Corner::TopLeft
        })
        .child(
            div()
                .flex()
                .flex_col()
                .pt(px(TRIGGER_HEIGHT + GAP))
                .pb(px(GAP))
                .child(card),
        );
    let at = div().absolute().top_0().left_0();
    deferred(
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(backdrop)
            .child(at.child(menu)),
    )
    .with_priority(1)
    .into_any_element()
}
