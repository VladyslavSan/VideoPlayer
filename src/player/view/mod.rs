//! Drawing the player overlay: a floating control bar, the panels above it,
//! the title, and transient messages. Everything on it fades in and out.

mod bar;
mod episodes_panel;
mod seek;
mod shortcuts;
mod track_panel;
mod up_next_card;
mod widgets;

use std::time::Instant;

use gpui::{AnyElement, Context, CursorStyle, MouseButton, Window, div, prelude::*, px};

use super::Player;
use super::fade::UpNextShown;
use super::messages::Holdup;
use super::panels::Panel;
use crate::motion;

const TEXT: u32 = 0xf4f5f7;
const MUTED: u32 = 0x9aa3b2;
const ACCENT: u32 = 0x4c8dff;
const SURFACE: u32 = 0x15171de8;
const BORDER: u32 = 0xffffff1a;

const BAR_HEIGHT: f32 = 52.0;
const BAR_BOTTOM: f32 = 24.0;
const BAR_MAX_WIDTH: f32 = 860.0;
const PANEL_WIDTH: f32 = 720.0;
/// Height kept clear of subtitles while the bar shows.
pub const BAR_CLEARANCE: f32 = BAR_BOTTOM + BAR_HEIGHT + 16.0;
/// Hit area of the sliders; the visible track is thinner.
const SLIDER_HEIGHT: f32 = 28.0;
/// Where toasts sit, from the top of the window.
const TOAST_TOP: f32 = 84.0;
/// The notice sits this far above the window's middle.
const NOTICE_RISE: f32 = 20.0;

pub fn format_speed(speed: f64) -> String {
    let text = format!("{speed:.2}");
    format!("{}×", text.trim_end_matches('0').trim_end_matches('.'))
}

fn season_name(season: u16) -> String {
    if season == 0 {
        "Specials".into()
    } else {
        format!("Season {season}")
    }
}

pub fn format_delay(seconds: f64) -> String {
    if seconds.abs() < 0.05 {
        "0.0 s".into()
    } else {
        format!("{seconds:+.1} s")
    }
}

impl Player {
    /// The control bar with the panels that open above it.
    fn bottom(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        div()
            .absolute()
            .bottom(px(BAR_BOTTOM))
            .left_0()
            .right_0()
            .px_6()
            .flex()
            .justify_center()
            .child(
                div()
                    .relative()
                    .w_full()
                    .max_w(px(BAR_MAX_WIDTH))
                    // Both can be on screen for a moment, one fading into the
                    // other.
                    .when(self.panel_drawn(Panel::Tracks), |d| {
                        let shown = self.fades.panel(Panel::Tracks).value();
                        d.child(self.track_panel(window, cx).opacity(shown))
                    })
                    .when(self.panel_drawn(Panel::Episodes), |d| {
                        let shown = self.fades.panel(Panel::Episodes).value();
                        d.child(self.episodes_panel(cx).opacity(shown))
                    })
                    .child(self.control_bar(window, cx)),
            )
            .into_any_element()
    }

    fn panel_drawn(&self, panel: Panel) -> bool {
        self.fades.panel(panel).on_screen()
    }

    /// What is holding the video up, for the notice in the middle.
    fn holdup(&self) -> Holdup {
        if self.stream.is_stalled() {
            Holdup::Reconnecting
        } else if self.status.buffering && !self.status.paused {
            Holdup::Loading
        } else {
            Holdup::Nothing
        }
    }

    /// Points each fade at where it should be now. Everything on the
    /// player that comes and goes does so through these.
    fn update_fades(&mut self, window: &Window) {
        self.fades.controls.set(self.controls_visible());
        self.fades.set_open_panel(self.panels.open());
        self.messages.expire_toast();
        self.messages.update_notice(self.holdup());
        let up_next = self
            .up_next
            .remaining(Instant::now())
            .filter(|_| self.playing.next().is_some());
        if let Some(seconds) = up_next {
            self.fades.shown.up_next = UpNextShown {
                seconds,
                ended: self.status.ended,
            };
        }
        self.fades.up_next.set(up_next.is_some());
        let skip = self.skip_target().filter(|_| up_next.is_none());
        if let Some(target) = skip {
            self.fades.shown.skip = target.label;
        }
        self.fades.skip.set(skip.is_some());
        let preview = self.sliders.seek.update_preview(self.status.duration);
        self.fades.preview.set(preview);
        let speed = (self.status.speed - 1.0).abs() > 0.01;
        if speed {
            self.fades.shown.speed = self.status.speed;
        }
        self.fades.speed.set(speed);
        self.fades.video.set(self.stream.video_shown());
        // Subtitles move out of the bar's way as it fades in.
        self.place_subtitles(window, self.fades.controls.value());
    }

    /// Whether anything is still on its way in or out, to keep drawing.
    fn fading(&self) -> bool {
        self.fades.moving()
            || self.messages.moving()
            || self.panels.switching(motion::OVERLAY.time())
    }

    /// The title and the control bar, while they're on screen.
    fn controls_layer(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.fades.controls.on_screen().then(|| {
            div()
                .absolute()
                .inset_0()
                .opacity(self.fades.controls.value())
                .child(self.top_bar(cx))
                .child(self.bottom(window, cx))
                .into_any_element()
        })
    }

    /// The notice in the middle and the toasts near the top, each with the
    /// ones still fading out.
    fn message_layers(&mut self, window: &Window) -> Vec<AnyElement> {
        let middle = f32::from(window.viewport_size().height) / 2.0 - NOTICE_RISE;
        let notices = self.messages.notice.drawn().into_iter();
        let toasts = self.messages.toasts.drawn().into_iter();
        notices
            .map(|(text, shown)| Self::centered_message(middle, text, shown))
            .chain(toasts.map(|(text, shown)| Self::centered_message(TOAST_TOP, text, shown)))
            .collect()
    }

    fn shortcuts_layer(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.panel_drawn(Panel::Shortcuts).then(|| {
            div()
                .absolute()
                .inset_0()
                .opacity(self.fades.panel(Panel::Shortcuts).value())
                .child(self.shortcuts_overlay(cx))
                .into_any_element()
        })
    }
}

impl Render for Player {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let visible = self.controls_visible();
        // The quality and sound menus live in the tracks panel and close
        // with it.
        if !self.panels.is_open(Panel::Tracks) {
            self.tracks.quality.menu.close();
            self.tracks.sound.menu.close();
        }
        self.update_fades(window);

        // No background: transparent pixels are where the video shows through.
        let mut overlay = Vec::new();
        overlay.extend(self.controls_layer(window, cx));
        overlay.extend(self.message_layers(window));
        overlay.extend(self.skip_layer(cx));
        overlay.extend(self.up_next_layer(cx));
        overlay.extend(self.shortcuts_layer(cx));
        if self.fading() {
            window.request_animation_frame();
        }
        // Black until the video is on screen, then it lifts off the video.
        let black = 1.0 - self.fades.video.value();

        div()
            .id("stage")
            .size_full()
            .relative()
            .when(black > 0.0, |d| d.bg(gpui::black().opacity(black)))
            .track_focus(&self.input.focus)
            .when(!visible, |d| d.cursor(CursorStyle::None))
            .on_key_down(cx.listener(Self::on_key))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_stage_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .children(overlay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speeds_drop_trailing_zeros() {
        assert_eq!(format_speed(1.0), "1×");
        assert_eq!(format_speed(1.5), "1.5×");
        assert_eq!(format_speed(1.75), "1.75×");
    }

    #[test]
    fn a_subtitle_delay_shows_its_sign_unless_it_is_nothing() {
        assert_eq!(format_delay(0.0), "0.0 s");
        assert_eq!(format_delay(0.04), "0.0 s");
        assert_eq!(format_delay(0.3), "+0.3 s");
        assert_eq!(format_delay(-1.2), "-1.2 s");
    }

    #[test]
    fn season_zero_is_the_specials() {
        assert_eq!(season_name(0), "Specials");
        assert_eq!(season_name(3), "Season 3");
    }
}
