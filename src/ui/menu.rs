//! Dropdown menus: a button showing the current choice, and a list of
//! choices that opens over everything else when it's clicked. Menus fade
//! in and out: the view keeps a `Shown` for each, which remembers a closed
//! menu until it has faded away.

use gpui::{
    AnchoredPositionMode, AnyElement, App, Corner, Div, ElementId, FontWeight, MouseButton,
    MouseDownEvent, Pixels, Point, SharedString, Stateful, Window, anchored, deferred, div, point,
    prelude::*, px, rgb, rgba,
};

use super::icons::{Icon, icon};
use crate::motion::Fade;

const TEXT: u32 = 0xf4f5f7;
const ACCENT: u32 = 0x4c8dff;
const SURFACE: u32 = 0x1c1f27;
const BORDER: u32 = 0xffffff1f;
/// Gap between the button and its menu.
const GAP: f32 = 4.0;
/// The trigger's height, which `popup` keeps clear when it places the menu.
const TRIGGER_HEIGHT: f32 = 32.0;

/// Which menu is open, if any, and the ones still fading out after they
/// closed. `T` says which menu (and what it's for): `()` for a view with
/// one menu.
pub struct Shown<T> {
    open: Option<Entry<T>>,
    closing: Vec<Entry<T>>,
    /// Numbers each menu as it opens, to tell copies apart.
    opened: usize,
}

struct Entry<T> {
    key: usize,
    value: T,
    fade: Fade,
}

/// A menu to draw, open or fading out.
pub struct Drawn<'a, T> {
    key: usize,
    pub value: &'a T,
    /// How visible it is: 0 (gone) to 1.
    shown: f32,
    closing: bool,
    moving: bool,
}

impl<T> Default for Shown<T> {
    fn default() -> Self {
        Self {
            open: None,
            closing: Vec::new(),
            opened: 0,
        }
    }
}

impl<T> Shown<T> {
    /// Opens `value`'s menu, closing any other.
    pub fn open(&mut self, value: T) {
        self.close();
        let mut fade = Fade::new(false);
        fade.set(true);
        self.opened += 1;
        self.open = Some(Entry {
            key: self.opened,
            value,
            fade,
        });
    }

    /// Starts the open menu fading out. Whether one was open.
    pub fn close(&mut self) -> bool {
        self.closing.retain(|entry| entry.fade.on_screen());
        let Some(mut entry) = self.open.take() else {
            return false;
        };
        entry.fade.set(false);
        self.closing.push(entry);
        true
    }

    /// The open menu's value.
    pub fn current(&self) -> Option<&T> {
        self.open.as_ref().map(|entry| &entry.value)
    }

    /// The menus to draw, the open one last (on top).
    pub fn drawn(&self) -> Vec<Drawn<'_, T>> {
        self.closing
            .iter()
            .chain(&self.open)
            .filter(|entry| entry.fade.on_screen())
            .map(|entry| Drawn {
                key: entry.key,
                value: &entry.value,
                shown: entry.fade.value(),
                closing: !entry.fade.shown(),
                moving: entry.fade.moving(),
            })
            .collect()
    }

    /// How lit a trigger should be: as visible as its menu, by `mine`.
    pub fn lit(&self, mine: impl Fn(&T) -> bool) -> f32 {
        self.drawn()
            .iter()
            .filter(|menu| mine(menu.value))
            .map(|menu| menu.shown)
            .fold(0.0, f32::max)
    }
}

/// The button that opens a menu: the current choice and a chevron. `lit`
/// (0 to 1) brightens it while its menu is open. The label is an element
/// so that it can dissolve when the choice changes.
pub fn trigger(id: impl Into<ElementId>, label: impl IntoElement, lit: f32) -> Stateful<Div> {
    let alpha = 0x14 + ((0x26 - 0x14) as f32 * lit.clamp(0.0, 1.0)).round() as u32;
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
        .bg(rgba(0xffffff00 | alpha))
        .hover(|s| s.bg(rgba(0xffffff26)))
        .cursor_pointer()
        .child(label)
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

/// A menu opened by a right-click, at `at` in the window. It opens down
/// and to the right of the point, or the other way where there's no room.
/// Clicking anywhere outside it calls `dismiss`.
pub fn context<T>(
    menu: &Drawn<'_, T>,
    at: Point<Pixels>,
    items: Vec<AnyElement>,
    window: &Window,
    dismiss: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let card = anchored()
        .position_mode(AnchoredPositionMode::Window)
        .position(at)
        .anchor(Corner::TopLeft)
        .child(surface(menu, items));
    deferred(
        div()
            .id(("menu-layer", menu.key))
            .children(backdrop(menu, window, dismiss))
            .child(card),
    )
    .with_priority(1)
    .into_any_element()
}

/// Catches clicks outside an open menu to close it. A closing menu has
/// none: the click that closed it is already handled, and the next one
/// goes to whatever is underneath.
fn backdrop<T>(
    menu: &Drawn<'_, T>,
    window: &Window,
    dismiss: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Option<AnyElement> {
    if menu.moving {
        window.request_animation_frame();
    }
    if menu.closing {
        return None;
    }
    let viewport = window.viewport_size();
    let dismiss = std::rc::Rc::new(dismiss);
    let close = || {
        let dismiss = dismiss.clone();
        move |event: &MouseDownEvent, window: &mut Window, cx: &mut App| {
            cx.stop_propagation();
            dismiss(event, window, cx);
        }
    };
    Some(
        anchored()
            .position(point(px(0.0), px(0.0)))
            .child(
                div()
                    .id("menu-backdrop")
                    .occlude()
                    .w(viewport.width)
                    .h(viewport.height)
                    .on_mouse_down(MouseButton::Left, close())
                    .on_mouse_down(MouseButton::Right, close()),
            )
            .into_any_element(),
    )
}

/// The menu's card, as visible as the menu. A closing one is covered so
/// its choices can't be hovered or clicked as it fades.
fn surface<T>(menu: &Drawn<'_, T>, items: Vec<AnyElement>) -> Div {
    div()
        .relative()
        .opacity(menu.shown)
        .child(card(items))
        .when(menu.closing, |d| {
            d.child(div().absolute().inset_0().occlude())
        })
}

fn card(items: Vec<AnyElement>) -> Stateful<Div> {
    div()
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
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .children(items)
}

/// The open menu, drawn on top of everything, below (or, with `up`, above)
/// the trigger. Place it in a `relative()` element that wraps the trigger.
/// If the menu doesn't fit on that side of the trigger it opens on the
/// other. Clicking anywhere outside it calls `dismiss`; the click goes no
/// further, so it doesn't also act on whatever is underneath.
pub fn popup<T>(
    menu: &Drawn<'_, T>,
    up: bool,
    items: Vec<AnyElement>,
    window: &Window,
    dismiss: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let backdrop = backdrop(menu, window, dismiss);
    let card = surface(menu, items);
    // Anchored at the trigger's top-left corner. GPUI flips an anchored
    // element to the other side of that point when it doesn't fit, so the
    // card is padded (with empty space that doesn't take clicks) by the
    // trigger's height above it and a gap below: hanging down from the
    // point it clears the trigger, and standing on it, it ends a gap above.
    let placed = anchored()
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
            .id(("menu-layer", menu.key))
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .children(backdrop)
            .child(at.child(placed)),
    )
    .with_priority(1)
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_menu_stays_until_it_has_faded() {
        let mut menus = Shown::default();
        assert!(!menus.close());
        menus.open(1);
        assert_eq!(menus.current(), Some(&1));
        // As if it had finished fading in.
        menus.open.as_mut().unwrap().fade = Fade::new(true);
        menus.open(2);
        assert_eq!(menus.current(), Some(&2));
        // Both drawn, the open one on top (unless motion is off).
        if crate::motion::enabled() {
            let drawn: Vec<_> = menus
                .drawn()
                .iter()
                .map(|m| (*m.value, m.closing))
                .collect();
            assert_eq!(drawn, [(1, true), (2, false)]);
        }
        assert!(menus.close());
        assert_eq!(menus.current(), None);
        assert!(menus.drawn().iter().all(|m| m.closing));
    }
}
