//! A server's Sonarr or Radarr in the sidebar, under its libraries: its
//! dot, its name, and how many downloads it has going.

use gpui::{AnyElement, ClickEvent, Context, SharedString, div, prelude::*, px, rgb, rgba};

use crate::arr::Kind;
use crate::arr::connections::Id;
use crate::shell::{ACCENT, MUTED, TEXT};

/// The apps' own colors, for their dots.
fn color(kind: Kind) -> u32 {
    match kind {
        Kind::Sonarr => 0x35c5f4,
        Kind::Radarr => 0xffc230,
    }
}

/// An app's dot, `size` across.
pub(super) fn dot(kind: Kind, size: f32) -> gpui::Div {
    div()
        .flex_none()
        .size(px(size))
        .rounded_full()
        .bg(rgb(color(kind)))
}

fn badge(count: usize) -> gpui::Div {
    div()
        .flex_none()
        .px_2()
        .rounded_full()
        .text_xs()
        .text_color(rgb(TEXT))
        .bg(rgb(ACCENT))
        .child(count.to_string())
}

/// An app's line in the sidebar.
pub(in crate::shell) struct Entry {
    pub app: Id,
    /// Its page is the tab that's open.
    pub selected: bool,
    /// How many downloads it has going.
    pub active: usize,
}

/// The app's line, for the view `V` that draws the sidebar; a click on it
/// calls `open`.
pub(in crate::shell) fn entry<V: 'static>(
    entry: Entry,
    cx: &mut Context<V>,
    open: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> AnyElement {
    let Entry {
        app: Id { server, kind },
        selected,
        active,
    } = entry;
    div()
        .id(SharedString::from(format!("side-{server}-{}", kind.name())))
        .flex()
        .flex_none()
        .items_center()
        .gap_2()
        .px_3()
        .pl_6()
        .py_1p5()
        .rounded_md()
        .cursor_pointer()
        .text_color(rgb(if selected { TEXT } else { MUTED }))
        .when(selected, |d| d.bg(rgba(0xffffff1a)))
        .when(!selected, |d| d.hover(|s| s.bg(rgba(0xffffff0d))))
        .child(dot(kind, 8.0))
        .child(div().flex_1().min_w(px(0.0)).truncate().child(kind.name()))
        .when(active > 0, |d| d.child(badge(active)))
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| open(this, cx)))
        .into_any_element()
}
