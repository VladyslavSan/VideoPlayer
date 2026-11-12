//! The Settings page: switches for app-wide preferences, kept in the
//! library database.

use gpui::{AnyElement, ClickEvent, Context, FontWeight, Window, div, prelude::*, px, rgb, rgba};

use super::{ACCENT, MUTED, Shell};

/// Whether headphone and media-key next/previous switch episodes.
const MEDIA_SKIP_SETTING: &str = "media_skip_buttons";

impl Shell {
    pub(super) fn media_skip_buttons(&self) -> bool {
        self.flag(MEDIA_SKIP_SETTING, true)
    }

    fn set_media_skip_buttons(&mut self, on: bool) {
        self.set_flag(MEDIA_SKIP_SETTING, on);
        if let Some(media) = &self.media {
            media.set_skip_buttons(on);
        }
    }

    fn flag(&self, key: &str, default: bool) -> bool {
        match self.db.setting(key).ok().flatten().as_deref() {
            Some("1") => true,
            Some("0") => false,
            _ => default,
        }
    }

    fn set_flag(&self, key: &str, on: bool) {
        if let Err(e) = self.db.set_setting(key, if on { "1" } else { "0" }) {
            eprintln!("[settings] saving {key}: {e:#}");
        }
    }

    pub(super) fn settings_page(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let skip = self.media_skip_buttons();
        vec![Self::section(
            "Headphones and media keys",
            div().flex().flex_col().gap_3().max_w(px(720.0)).child(switch_row(
                "setting-media-skip",
                "Next and previous switch episodes",
                "When off, the skip buttons on headphones and keyboards do nothing in the player. \
                 Play and pause always work.",
                skip,
                cx,
                move |this, _, cx| {
                    this.set_media_skip_buttons(!skip);
                    cx.notify();
                },
            )),
        )]
    }
}

/// A setting that's on or off: the text on the left, a switch on the right;
/// clicking anywhere on the row flips it.
fn switch_row(
    id: &'static str,
    title: &'static str,
    description: &'static str,
    on: bool,
    cx: &mut Context<Shell>,
    toggle: impl Fn(&mut Shell, &mut Window, &mut Context<Shell>) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_between()
        .gap_6()
        .px_4()
        .py_3()
        .rounded_lg()
        .bg(rgba(0xffffff0a))
        .hover(|s| s.bg(rgba(0xffffff14)))
        .cursor_pointer()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .min_w(px(0.0))
                .child(div().font_weight(FontWeight::MEDIUM).child(title))
                .child(div().text_sm().text_color(rgb(MUTED)).child(description)),
        )
        .child(
            div()
                .flex_none()
                .w(px(40.0))
                .h(px(22.0))
                .p(px(2.0))
                .rounded_full()
                .bg(if on { rgb(ACCENT) } else { rgba(0xffffff2e) })
                .flex()
                .when(on, |d| d.justify_end())
                .child(div().size(px(18.0)).rounded_full().bg(rgb(0xffffff))),
        )
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| toggle(this, window, cx)))
        .into_any_element()
}
