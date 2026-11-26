//! Small pieces every page is built from: buttons, a labelled text field, a
//! titled section, a wrapping grid of cards. They hold no state; a button
//! calls back into the view that draws it.

use gpui::{
    AnyElement, ClickEvent, Context, ElementId, Entity, FontWeight, SharedString, Window, div,
    prelude::*, rgb, rgba,
};

use super::{ACCENT, MUTED, TEXT};
use crate::ui::text_input::TextInput;

/// A small button, as on the bars and in Settings.
pub(super) fn button<V: 'static>(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    cx: &mut Context<V>,
    action: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) -> AnyElement {
    div()
        .id(id.into())
        .flex_none()
        .px_3()
        .py_1p5()
        .rounded_md()
        .text_sm()
        .bg(rgba(0xffffff14))
        .hover(|s| s.bg(rgba(0xffffff26)))
        .cursor_pointer()
        .child(label.into())
        .on_click(cx.listener(move |view, _: &ClickEvent, window, cx| action(view, window, cx)))
        .into_any_element()
}

/// How an action button stands out.
#[derive(Clone, Copy)]
enum Weight {
    /// The page's main action (Play): light on the dark page.
    Primary,
    Plain,
}

/// The main action on a details page, as a light pill.
pub(super) fn primary_action_button<V: 'static>(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    cx: &mut Context<V>,
    action: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) -> AnyElement {
    pill(id.into(), label.into(), Weight::Primary)
        .on_click(cx.listener(move |view, _: &ClickEvent, window, cx| action(view, window, cx)))
        .into_any_element()
}

/// Another action on a details page, as a dim pill.
pub(super) fn action_button<V: 'static>(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    cx: &mut Context<V>,
    action: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) -> AnyElement {
    pill(id.into(), label.into(), Weight::Plain)
        .on_click(cx.listener(move |view, _: &ClickEvent, window, cx| action(view, window, cx)))
        .into_any_element()
}

fn pill(id: ElementId, label: SharedString, weight: Weight) -> gpui::Stateful<gpui::Div> {
    let (background, text) = match weight {
        Weight::Primary => (0xf4f5f7ff, 0x101218),
        Weight::Plain => (0xffffff1f, TEXT),
    };
    div()
        .id(id)
        .px_5()
        .py_2()
        .rounded_full()
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .bg(rgba(background))
        .text_color(rgb(text))
        .hover(|s| s.opacity(0.85))
        .child(label)
}

/// A text input under its label.
pub(super) fn field(label: &str, input: &Entity<TextInput>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_1p5()
        .child(
            div()
                .text_sm()
                .text_color(rgb(MUTED))
                .child(label.to_string()),
        )
        .child(input.clone())
        .into_any_element()
}

/// A row of choices, one picked. `pick` gets the index of the one clicked.
pub(super) fn chips<V: 'static>(
    id: &str,
    title: &str,
    options: Vec<(String, bool)>,
    cx: &mut Context<V>,
    pick: impl Fn(&mut V, usize) + Clone + 'static,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_1p5()
        .child(
            div()
                .text_sm()
                .text_color(rgb(MUTED))
                .child(title.to_string()),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .children(options.into_iter().enumerate().map(|(i, (label, on))| {
                    let pick = pick.clone();
                    div()
                        .id(SharedString::from(format!("{id}-{i}")))
                        .px_3()
                        .py_1()
                        .rounded_md()
                        .text_sm()
                        .cursor_pointer()
                        .bg(if on { rgb(ACCENT) } else { rgba(0xffffff12) })
                        .when(!on, |d| d.hover(|s| s.bg(rgba(0xffffff26))))
                        .child(label)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            pick(this, i);
                            cx.notify();
                        }))
                }))
                .into_any_element(),
        )
        .into_any_element()
}

/// A part of a page under its title.
pub(super) fn section(title: &str, body: impl IntoElement) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .pt_6()
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.to_string()),
        )
        .child(body)
        .into_any_element()
}

/// Cards side by side, wrapping onto as many rows as they need.
pub(super) fn grid(cards: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_wrap()
        .gap_5()
        .children(cards)
        .into_any_element()
}
