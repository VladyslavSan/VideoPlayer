//! Settings → Playback: the sound filters (`sound_page.rs`) and whether the
//! media keys' next and previous switch episodes.

use gpui::{AnyElement, Context};

use super::{Event, Settings};
use crate::shell::switches::SwitchRow;
use crate::shell::widgets;

const MEDIA_SKIP_SWITCH: &str = "setting-media-skip";

impl Settings {
    /// Saves the switch and tells the shell, which owns the media controls.
    fn set_media_skip_buttons(&mut self, on: bool, cx: &mut Context<Self>) {
        self.preferences().set_media_skip_buttons(on);
        cx.emit(Event::MediaSkipButtonsChanged { on });
        cx.notify();
    }

    pub(super) fn playback_page(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let skips = self.preferences().media_skip_buttons();
        let media_keys = SwitchRow {
            id: MEDIA_SKIP_SWITCH,
            title: "Next and previous switch episodes",
            description: "When off, the skip buttons on headphones and keyboards do nothing in the \
                          player. Play and pause always work.",
            on: skips,
        };
        let media_keys = self
            .switches
            .row(media_keys, cx, move |this: &mut Self, _, cx| {
                this.set_media_skip_buttons(!skips, cx)
            });
        vec![
            widgets::section("Sound", self.sound_entry(cx)),
            widgets::section("Headphones and media keys", media_keys),
        ]
    }
}
