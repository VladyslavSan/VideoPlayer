//! The control bar along the bottom and the title along the top.

use gpui::{
    AnyElement, ClickEvent, Context, Div, FontWeight, MouseButton, Window, canvas, div, prelude::*,
    px, relative, rgb, rgba,
};

use super::widgets::{HOVER, chip, lit};
use super::{BAR_HEIGHT, MUTED, SLIDER_HEIGHT, TEXT, format_speed};
use crate::player::panels::Panel;
use crate::player::{Player, format_time};
use crate::ui::icons::Icon;

/// Room the speed chip ("1.75×") takes in the control bar.
const SPEED_CHIP_WIDTH: f32 = 60.0;
/// The sliders' unfilled track.
pub(super) const TRACK: u32 = 0xffffff2e;

/// A time beside the seek bar.
fn time_label(text: String) -> Div {
    div()
        .flex_none()
        .min_w(px(52.0))
        .flex()
        .justify_center()
        .text_sm()
        .text_color(rgb(TEXT))
        .child(text)
}

impl Player {
    /// Circular-arrow button with the skip length written inside.
    fn skip_button(
        &self,
        id: &'static str,
        glyph: Icon,
        seconds: f64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.icon_button(id, glyph, 26.0, cx, move |this, _, _| this.skip(seconds))
            .relative()
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .pt(px(1.5))
                    .text_size(px(8.5))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(TEXT))
                    .child(format!("{}", seconds.abs() as u32)),
            )
            .into_any_element()
    }

    /// The back button and the title, along the top.
    pub(super) fn top_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let back = self.icon_button("back", Icon::Back, 22.0, cx, |this, _, cx| this.exit(cx));
        div()
            .absolute()
            .top_5()
            .left_5()
            .right_5()
            .flex()
            .items_center()
            .gap_3()
            .child(chip().child(back))
            .child(
                chip()
                    .px_4()
                    .py_1p5()
                    .flex()
                    .items_baseline()
                    .gap_2()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.playing.title.clone()),
                    )
                    .children(
                        self.playing
                            .subtitle
                            .clone()
                            .map(|s| div().text_sm().text_color(rgb(MUTED)).child(s)),
                    ),
            )
            .into_any_element()
    }

    fn volume_slider(&self, cx: &mut Context<Self>) -> AnyElement {
        let level = if self.status.muted {
            0.0
        } else {
            (self.status.volume / 100.0) as f32
        };
        let bounds = self.sliders.volume_bounds_cell();
        div()
            .id("volume")
            .w(px(72.0))
            .flex_none()
            .h(px(SLIDER_HEIGHT))
            .relative()
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_volume_down))
            .child(
                canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px((SLIDER_HEIGHT - 4.0) / 2.0))
                    .h(px(4.0))
                    .rounded_full()
                    .bg(rgba(TRACK))
                    .child(
                        div()
                            .h_full()
                            .w(relative(level))
                            .rounded_full()
                            .bg(rgb(TEXT)),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .top(px((SLIDER_HEIGHT - 10.0) / 2.0))
                    .left(relative(level))
                    .ml(px(-5.0))
                    .size(px(10.0))
                    .rounded_full()
                    .bg(rgb(TEXT)),
            )
            .into_any_element()
    }

    pub(super) fn control_bar(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let status = self.status;
        let shown = self.sliders.seek.shown(status.position);
        let volume_icon = if status.muted || status.volume <= 0.0 {
            Icon::VolumeMuted
        } else {
            Icon::Volume
        };
        chip()
            .id("bar")
            .h(px(BAR_HEIGHT))
            .w_full()
            .px_1p5()
            .flex()
            .items_center()
            .gap_0p5()
            // Clicks on the bar itself must not pause the video underneath.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.activity.set_bar_hovered(*hovered);
                cx.notify();
            }))
            .children(self.transport_buttons(cx))
            .child(time_label(format_time(shown)))
            .child(self.seek_bar(cx))
            .child(time_label(format!(
                "-{}",
                format_time(status.duration - shown)
            )))
            .child(
                self.icon_button("mute", volume_icon, 22.0, cx, |this, _, _| {
                    this.toggle_mute()
                }),
            )
            .child(self.volume_slider(cx))
            .child(div().w(px(6.0)).flex_none())
            .children(self.speed_slot(cx))
            .children(self.panel_buttons(window, cx))
            .into_any_element()
    }

    /// Play/pause, the two skips and, for a show, the next episode.
    fn transport_buttons(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let play_icon = if self.status.paused {
            Icon::Play
        } else {
            Icon::Pause
        };
        let controls = &self.input.controls;
        let mut buttons = vec![
            self.icon_button("play", play_icon, 24.0, cx, |this, _, _| {
                this.toggle_pause()
            })
            .into_any_element(),
            self.skip_button("skip-back", Icon::SkipBack, -controls.back_seconds, cx),
            self.skip_button(
                "skip-forward",
                Icon::SkipForward,
                controls.forward_seconds,
                cx,
            ),
        ];
        if self.playing.next().is_some() {
            let next = self.icon_button(
                "next-episode",
                Icon::NextEpisode,
                20.0,
                cx,
                |this, _, cx| this.play_next(cx),
            );
            buttons.push(next.into_any_element());
        }
        buttons
    }

    /// The speed chip, while it's on screen. Its room opens up as it fades
    /// in, rather than shoving the seek bar aside.
    fn speed_slot(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let speed = self.fades.speed.value();
        self.fades.speed.on_screen().then(|| {
            div()
                .flex_none()
                .w(px(SPEED_CHIP_WIDTH * speed))
                .overflow_hidden()
                .opacity(speed)
                .flex()
                .justify_center()
                .child(self.speed_chip(cx))
                .into_any_element()
        })
    }

    /// The buttons at the bar's right end: episodes (for a show), tracks and
    /// fullscreen.
    fn panel_buttons(&self, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let fullscreen_icon = if window.is_fullscreen() {
            Icon::FullscreenExit
        } else {
            Icon::Fullscreen
        };
        let mut buttons = Vec::new();
        if self.playing.has_episodes() {
            let episodes = self.icon_button("episodes", Icon::Episodes, 21.0, cx, |this, _, _| {
                this.toggle_panel(Panel::Episodes)
            });
            let active = self.fades.panel(Panel::Episodes).value();
            buttons.push(lit(episodes, active).into_any_element());
        }
        let tracks = self.icon_button("tracks", Icon::Subtitles, 22.0, cx, |this, _, _| {
            this.toggle_panel(Panel::Tracks)
        });
        let active = self.fades.panel(Panel::Tracks).value();
        buttons.push(lit(tracks, active).into_any_element());
        let fullscreen =
            self.icon_button("fullscreen", fullscreen_icon, 20.0, cx, |_, window, _| {
                window.toggle_fullscreen()
            });
        buttons.push(fullscreen.into_any_element());
        buttons
    }

    /// The playback speed, when it isn't normal; opens the track panel.
    fn speed_chip(&self, cx: &mut Context<Self>) -> AnyElement {
        // While it fades out at normal speed again, it keeps its last value.
        let speed = self.fades.shown.speed;
        div()
            .id("speed")
            .flex_none()
            .px_2()
            .py_1()
            .rounded_full()
            .text_sm()
            .whitespace_nowrap()
            .cursor_pointer()
            .hover(|s| s.bg(rgba(HOVER)))
            .child(
                self.fades
                    .swaps
                    .draw("speed", format_speed(speed), |text| div().child(text)),
            )
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.toggle_panel(Panel::Tracks);
                this.poke(cx);
            }))
            .into_any_element()
    }
}
