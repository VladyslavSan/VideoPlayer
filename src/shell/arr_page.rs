//! A server's Sonarr or Radarr, in the sidebar under its libraries: a page
//! for adding shows or movies to it by name, and for what it's downloading
//! and has lately done.

use gpui::{
    AnyElement, ClickEvent, Context, Entity, FontWeight, SharedString, div, prelude::*, px, rgb,
    rgba,
};

use super::{ACCENT, MUTED, Screen, Shell, TEXT};
use crate::arr::Kind;
use crate::ui::text_input::TextInput;

/// The apps' own colors, for their dots.
fn color(kind: Kind) -> u32 {
    match kind {
        Kind::Sonarr => 0x35c5f4,
        Kind::Radarr => 0xffc230,
    }
}

fn dot(kind: Kind, size: f32) -> gpui::Div {
    div()
        .flex_none()
        .size(px(size))
        .rounded_full()
        .bg(rgb(color(kind)))
}

impl Shell {
    /// The name field on the app's page.
    pub(super) fn arr_input(&self, kind: Kind) -> &Entity<TextInput> {
        &self.arr_search[match kind {
            Kind::Sonarr => 0,
            Kind::Radarr => 1,
        }]
    }

    /// The app's line in the sidebar: its dot, its name, and how many
    /// downloads it has going.
    pub(super) fn arr_entry(&self, server: i64, kind: Kind, cx: &mut Context<Self>) -> AnyElement {
        let screen = Screen::Arr(server, kind);
        let selected = self.tab == screen;
        let active = self.downloads.active_for((server, kind));
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
            .when(active > 0, |d| {
                d.child(
                    div()
                        .flex_none()
                        .px_2()
                        .rounded_full()
                        .text_xs()
                        .text_color(rgb(TEXT))
                        .bg(rgb(ACCENT))
                        .child(active.to_string()),
                )
            })
            .on_click(
                cx.listener(move |this, _: &ClickEvent, _, cx| this.set_tab(screen.clone(), cx)),
            )
            .into_any_element()
    }

    /// Opening the page starts with an empty name field.
    pub(super) fn arr_page_opened(&mut self, kind: Kind, cx: &mut Context<Self>) {
        self.arr_input(kind)
            .clone()
            .update(cx, |input, cx| input.set_text("", cx));
        self.refresh_downloads(cx);
    }

    pub(super) fn arr_page(
        &self,
        server: i64,
        kind: Kind,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let noun = match kind {
            Kind::Sonarr => "show",
            Kind::Radarr => "movie",
        };
        let title = div()
            .pt_6()
            .flex()
            .items_center()
            .gap_3()
            .child(dot(kind, 14.0))
            .child(
                div()
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .child(kind.name()),
            )
            .child(
                div()
                    .text_color(rgb(MUTED))
                    .child(self.source_name(Some(server))),
            )
            .into_any_element();
        let query = self.arr_input(kind).read(cx).text().trim().to_string();
        let results: Vec<AnyElement> = if self.finder_shows((server, kind), &query) {
            self.finder_rows((server, kind), cx)
        } else if query.is_empty() {
            vec![
                div()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child(format!(
                        "Type a {noun}'s name to find it and add it to {}.",
                        kind.name()
                    ))
                    .into_any_element(),
            ]
        } else {
            Vec::new()
        };
        let add = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .w(px(420.0))
                    .max_w_full()
                    .child(self.arr_input(kind).clone()),
            )
            .children(results);
        let mut page = vec![title, Self::section(&format!("Add a {noun}"), add)];
        page.extend(self.app_activity((server, kind), cx));
        page
    }
}
