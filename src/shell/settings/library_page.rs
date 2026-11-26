//! Settings → Library: the Jellyfin servers signed in to (each with its
//! streaming quality, its Sonarr and Radarr, and Sign Out), the sign-in
//! form, and the local folders.

use std::path::Path;

use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, MouseDownEvent, SharedString, Window, div,
    prelude::*, px, rgb, rgba,
};

use super::{Settings, line, note, row};
use crate::jellyfin::{QUALITIES, Quality, Server};
use crate::shell::{self, MUTED, widgets};
use crate::ui::menu;

/// A server's streaming quality menu, open or fading out.
pub(super) struct QualityMenu {
    server: i64,
    /// The quality in use when the menu opened: checked even as it fades out.
    checked: Quality,
}

impl Settings {
    fn sign_out(&mut self, server_id: i64, cx: &mut Context<Self>) {
        if let Some(server) = self.catalog.server(server_id).cloned() {
            self.connections
                .update(cx, |connections, cx| connections.forget(&server, cx));
        }
        self.update_library(cx, |library, cx| library.sign_out(server_id, cx));
        cx.notify();
    }

    fn remove_folder(&mut self, root: &Path, cx: &mut Context<Self>) {
        self.update_library(cx, |library, cx| library.remove_folder(root, cx));
        cx.notify();
    }

    fn toggle_quality_menu(&mut self, server: i64, cx: &mut Context<Self>) {
        let is_open = self
            .quality_menu
            .current()
            .is_some_and(|menu| menu.server == server);
        if is_open {
            self.quality_menu.close();
        } else {
            let checked = self.preferences().quality(server);
            self.quality_menu.open(QualityMenu { server, checked });
        }
        cx.notify();
    }

    fn pick_quality(&mut self, server: i64, quality: Quality, cx: &mut Context<Self>) {
        self.preferences().set_quality(server, quality);
        self.close_quality_menu(cx);
    }

    pub(super) fn close_quality_menu(&mut self, cx: &mut Context<Self>) {
        self.quality_menu.close();
        cx.notify();
    }

    pub(super) fn library_page(&self, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        vec![
            widgets::section("Jellyfin servers", self.servers(window, cx)),
            widgets::section("Folders", self.folders(cx)),
        ]
    }

    /// The servers signed in to, then the sign-in form or the button opening it.
    fn servers(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let servers: Vec<AnyElement> = self
            .catalog
            .servers()
            .iter()
            .map(|server| self.server_card(server, window, cx))
            .collect();
        let sign_in = if self.sign_in.is_open() {
            self.sign_in_form(cx)
        } else {
            div()
                .flex()
                .pt_1()
                .child(widgets::button(
                    "add-server",
                    "Add Server",
                    cx,
                    |this: &mut Self, window, cx| this.open_sign_in(window, cx),
                ))
                .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(note(
                "Movies and shows on these servers appear in your library. You can sign in to \
                 several servers, or to one server as different users. Each server can have \
                 its own Sonarr and Radarr, for requesting what it doesn't have yet.",
            ))
            .children(servers)
            .child(sign_in)
    }

    /// One server: who is signed in where, its quality and Sign Out, then
    /// its Sonarr and Radarr.
    fn server_card(&self, server: &Server, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let id = server.id;
        let account = div()
            .flex()
            .flex_col()
            .min_w(px(0.0))
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .child(server.name.clone()),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .truncate()
                    .child(format!("{} · {}", server.user_name, server.url)),
            );
        let buttons = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .child(self.quality_picker(id, window, cx))
            .child(widgets::button(
                SharedString::from(format!("sign-out-{id}")),
                "Sign Out",
                cx,
                move |this: &mut Self, _, cx| this.sign_out(id, cx),
            ));
        div()
            .flex()
            .flex_col()
            .rounded_lg()
            .bg(rgba(0xffffff0a))
            .child(line(account).child(buttons))
            .child(self.server_apps(server, cx))
            .into_any_element()
    }

    /// "Streaming" and a button that opens a menu of quality presets.
    fn quality_picker(&self, server: i64, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let current = self.preferences().quality(server);
        let mine = move |menu: &QualityMenu| menu.server == server;
        let label = self.motion.changing_text(
            format!("quality-{server}"),
            format!("Streaming: {}", current.label()),
            |label| div().child(label),
        );
        let trigger = menu::trigger(
            SharedString::from(format!("quality-{server}")),
            label,
            self.quality_menu.lit(mine),
        )
        .on_click(
            cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_quality_menu(server, cx)),
        );
        let mut picker = div().relative().flex().child(trigger);
        for shown in self.quality_menu.drawn().iter().filter(|m| mine(m.value)) {
            let items = quality_items(server, shown.value.checked, cx);
            let dismiss =
                cx.listener(|this, _: &MouseDownEvent, _, cx| this.close_quality_menu(cx));
            picker = picker.child(menu::popup(shown, false, items, window, dismiss));
        }
        picker.into_any_element()
    }

    /// The local folders, each with Remove, then Add Folder.
    fn folders(&self, cx: &mut Context<Self>) -> gpui::Div {
        let folders: Vec<AnyElement> = self
            .catalog
            .roots()
            .iter()
            .enumerate()
            .map(|(index, root)| {
                let path = root.clone();
                row(div()
                    .min_w(px(0.0))
                    .truncate()
                    .child(root.display().to_string()))
                .child(widgets::button(
                    SharedString::from(format!("remove-folder-{index}")),
                    "Remove",
                    cx,
                    move |this: &mut Self, _, cx| this.remove_folder(&path, cx),
                ))
                .into_any_element()
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(note(
                "Videos in these folders, and folders inside them, appear in your library. \
                 Removing a folder takes its videos and their progress out of the library; \
                 the files stay on disk.",
            ))
            .children(folders)
            .child(div().flex().pt_1().child(widgets::button(
                "add-folder",
                "Add Folder",
                cx,
                |this: &mut Self, _, cx| shell::pick_folders(this.library.clone(), cx),
            )))
    }
}

/// The menu's choices, the one in use when it opened checked.
fn quality_items(server: i64, checked: Quality, cx: &mut Context<Settings>) -> Vec<AnyElement> {
    QUALITIES
        .iter()
        .enumerate()
        .map(|(index, &quality)| {
            menu::item(
                ("quality-choice", index),
                quality.menu_label(),
                quality == checked,
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.pick_quality(server, quality, cx)
            }))
            .into_any_element()
        })
        .collect()
}
