//! Drawing the player overlay: a floating control bar, the audio/subtitle
//! panel above it, the title, and transient messages.

use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, ElementId, FontWeight, MouseButton,
    MouseDownEvent, ObjectFit, SharedString, Window, canvas, div, img, prelude::*, px, relative,
    rgb, rgba,
};

use super::{Panel, Player, TOAST_DURATION, tracks};
use crate::ui::icons::{Icon, icon};
use crate::video::TrackKind;

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

fn chip() -> gpui::Div {
    div()
        .rounded_full()
        .bg(rgba(SURFACE))
        .border_1()
        .border_color(rgba(BORDER))
        .text_color(rgb(TEXT))
}

impl Player {
    fn icon_button(
        &self,
        id: impl Into<ElementId>,
        glyph: Icon,
        size: f32,
        active: bool,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .size(px(40.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .cursor_pointer()
            .when(active, |d| d.bg(rgba(0xffffff26)))
            .hover(|s| s.bg(rgba(0xffffff1f)))
            .child(icon(glyph).size(px(size)).text_color(rgb(TEXT)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    action(this, window, cx);
                    this.poke(cx);
                }),
            )
    }

    /// Circular-arrow button with the skip length written inside.
    fn skip_button(
        &self,
        id: &'static str,
        glyph: Icon,
        seconds: f64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.icon_button(id, glyph, 26.0, false, cx, move |this, _, _| {
            this.skip(seconds)
        })
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

    fn top_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .absolute()
            .top_5()
            .left_5()
            .right_5()
            .flex()
            .items_center()
            .gap_3()
            .child(chip().child(self.icon_button(
                "back",
                Icon::Back,
                22.0,
                false,
                cx,
                |this, _, cx| this.exit(cx),
            )))
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
                            .child(self.title.clone()),
                    )
                    .children(
                        self.subtitle
                            .clone()
                            .map(|s| div().text_sm().text_color(rgb(MUTED)).child(s)),
                    ),
            )
            .into_any_element()
    }

    fn seek_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let duration = self.status.duration;
        let fraction = |t: f64| {
            if duration > 0.0 {
                (t / duration).clamp(0.0, 1.0) as f32
            } else {
                0.0
            }
        };
        let progress = fraction(self.scrub.unwrap_or(self.status.position));
        let buffered = self.status.buffered.map(fraction);
        let hovered = self.seek_hover.is_some();
        let track_height = if hovered { 6.0 } else { 4.0 };
        let bounds = self.seek_bounds.clone();

        let preview = self.seek_hover.filter(|_| duration > 0.0).map(|x| {
            let area = self.seek_bounds.get();
            let time = super::fraction_at(area, x) * duration;
            let offset = f32::from(x - area.origin.x).clamp(0.0, f32::from(area.size.width));
            div()
                .absolute()
                .bottom(px(SLIDER_HEIGHT + 6.0))
                .left(px(offset - 32.0))
                .w(px(64.0))
                .flex()
                .justify_center()
                .child(
                    chip()
                        .px_2()
                        .py_0p5()
                        .text_xs()
                        .child(super::format_time(time)),
                )
        });

        div()
            .id("seek")
            .flex_1()
            .min_w(px(80.0))
            .h(px(SLIDER_HEIGHT))
            .relative()
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_seek_down))
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
                    .top(px((SLIDER_HEIGHT - track_height) / 2.0))
                    .h(px(track_height))
                    .rounded_full()
                    .bg(rgba(0xffffff2e))
                    .children(buffered.map(|b| {
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .h_full()
                            .w(relative(b))
                            .rounded_full()
                            .bg(rgba(0xffffff38))
                    }))
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .h_full()
                            .w(relative(progress))
                            .rounded_full()
                            .bg(rgb(ACCENT)),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .top(px((SLIDER_HEIGHT - 12.0) / 2.0))
                    .left(relative(progress))
                    .ml(px(-6.0))
                    .size(px(12.0))
                    .rounded_full()
                    .bg(rgb(TEXT)),
            )
            .children(preview)
            .into_any_element()
    }

    fn volume_slider(&self, cx: &mut Context<Self>) -> AnyElement {
        let level = if self.status.muted {
            0.0
        } else {
            (self.status.volume / 100.0) as f32
        };
        let bounds = self.volume_bounds.clone();
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
                    .bg(rgba(0xffffff2e))
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

    fn control_bar(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let status = self.status;
        let shown = self.scrub.unwrap_or(status.position);
        let time = |text: String| {
            div()
                .flex_none()
                .min_w(px(52.0))
                .flex()
                .justify_center()
                .text_sm()
                .text_color(rgb(TEXT))
                .child(text)
        };
        let play_icon = if status.paused {
            Icon::Play
        } else {
            Icon::Pause
        };
        let volume_icon = if status.muted || status.volume <= 0.0 {
            Icon::VolumeMuted
        } else {
            Icon::Volume
        };
        let fullscreen_icon = if window.is_fullscreen() {
            Icon::FullscreenExit
        } else {
            Icon::Fullscreen
        };
        let speed = (status.speed - 1.0).abs() > 0.01;

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
                this.bar_hovered = *hovered;
                cx.notify();
            }))
            .child(
                self.icon_button("play", play_icon, 24.0, false, cx, |this, _, _| {
                    this.toggle_pause()
                }),
            )
            .child(self.skip_button("skip-back", Icon::SkipBack, -super::SKIP_SECONDS, cx))
            .child(self.skip_button("skip-forward", Icon::SkipForward, super::SKIP_SECONDS, cx))
            .when(self.next_episode().is_some(), |bar| {
                bar.child(self.icon_button(
                    "next-episode",
                    Icon::NextEpisode,
                    20.0,
                    false,
                    cx,
                    |this, _, cx| this.play_next(cx),
                ))
            })
            .child(time(super::format_time(shown)))
            .child(self.seek_bar(cx))
            .child(time(format!(
                "-{}",
                super::format_time(status.duration - shown)
            )))
            .child(
                self.icon_button("mute", volume_icon, 22.0, false, cx, |this, _, _| {
                    this.toggle_mute()
                }),
            )
            .child(self.volume_slider(cx))
            .child(div().w(px(6.0)).flex_none())
            .when(speed, |bar| {
                bar.child(
                    div()
                        .id("speed")
                        .flex_none()
                        .px_2()
                        .py_1()
                        .rounded_full()
                        .text_sm()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgba(0xffffff1f)))
                        .child(format_speed(status.speed))
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.toggle_panel(Panel::Tracks);
                            this.poke(cx);
                        })),
                )
            })
            .when(!self.episodes.is_empty(), |bar| {
                bar.child(self.icon_button(
                    "episodes",
                    Icon::Episodes,
                    21.0,
                    self.panel == Some(Panel::Episodes),
                    cx,
                    |this, _, _| this.toggle_panel(Panel::Episodes),
                ))
            })
            .child(self.icon_button(
                "tracks",
                Icon::Subtitles,
                22.0,
                self.panel == Some(Panel::Tracks),
                cx,
                |this, _, _| this.toggle_panel(Panel::Tracks),
            ))
            .child(self.icon_button(
                "fullscreen",
                fullscreen_icon,
                20.0,
                false,
                cx,
                |_, window, _| window.toggle_fullscreen(),
            ))
            .into_any_element()
    }

    fn panel_column(title: &'static str, rows: Vec<AnyElement>) -> AnyElement {
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .px_2()
                    .pb_1()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(MUTED))
                    .child(title),
            )
            .child(
                div()
                    .id(title)
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .max_h(px(280.0))
                    .overflow_y_scroll()
                    .children(rows),
            )
            .into_any_element()
    }

    fn choice_row(
        id: ElementId,
        primary: SharedString,
        secondary: SharedString,
        selected: bool,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        div()
            .id(id)
            .flex()
            .items_start()
            .gap_2()
            .px_2()
            .py_1p5()
            .rounded_lg()
            .cursor_pointer()
            .hover(|s| s.bg(rgba(0xffffff14)))
            .child(
                div()
                    .size(px(18.0))
                    .flex_none()
                    .pt_0p5()
                    .when(selected, |d| {
                        d.child(icon(Icon::Check).size(px(16.0)).text_color(rgb(ACCENT)))
                    }),
            )
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().text_sm().child(primary))
                    .when(!secondary.is_empty(), |d| {
                        d.child(div().text_xs().text_color(rgb(MUTED)).child(secondary))
                    }),
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                action(this, cx);
                this.poke(cx);
            }))
            .into_any_element()
    }

    fn stepper(
        &self,
        id: &'static str,
        label: &'static str,
        value: String,
        cx: &mut Context<Self>,
        step: impl Fn(&mut Self, i32, &mut Context<Self>) + Clone + 'static,
    ) -> AnyElement {
        let down = step.clone();
        let small = |button: gpui::Stateful<gpui::Div>| button.size(px(30.0));
        div()
            .flex()
            .flex_col()
            .gap_1()
            .px_2()
            .py_1()
            .child(div().text_xs().text_color(rgb(MUTED)).child(label))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(small(self.icon_button(
                        (id, 0usize),
                        Icon::Minus,
                        16.0,
                        false,
                        cx,
                        move |this, _, cx| down(this, -1, cx),
                    )))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .justify_center()
                            .text_sm()
                            .child(value),
                    )
                    .child(small(self.icon_button(
                        (id, 1usize),
                        Icon::Plus,
                        16.0,
                        false,
                        cx,
                        move |this, _, cx| step(this, 1, cx),
                    ))),
            )
            .into_any_element()
    }

    fn track_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut audio = Vec::new();
        for (i, track) in self.tracks_of(TrackKind::Audio).enumerate() {
            let label = tracks::label(track, i + 1);
            let id = track.id;
            audio.push(Self::choice_row(
                ElementId::NamedInteger("audio".into(), id as u64),
                label.primary.into(),
                label.secondary.into(),
                track.selected,
                cx,
                move |this, cx| this.select_audio(id, cx),
            ));
        }
        if audio.is_empty() {
            audio.push(
                div()
                    .px_2()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child("None")
                    .into_any_element(),
            );
        }

        let any_subtitle = self.tracks_of(TrackKind::Subtitle).any(|t| t.selected);
        let mut subtitles = vec![Self::choice_row(
            "subtitle-off".into(),
            "Off".into(),
            SharedString::default(),
            !any_subtitle,
            cx,
            |this, cx| this.select_subtitle(None, cx),
        )];
        for (i, track) in self.tracks_of(TrackKind::Subtitle).enumerate() {
            let label = tracks::label(track, i + 1);
            let id = track.id;
            subtitles.push(Self::choice_row(
                ElementId::NamedInteger("subtitle".into(), id as u64),
                label.primary.into(),
                label.secondary.into(),
                track.selected,
                cx,
                move |this, cx| this.select_subtitle(Some(id), cx),
            ));
        }

        let status = self.status;
        let options = vec![
            self.stepper(
                "speed",
                "Speed",
                format_speed(status.speed),
                cx,
                |this, d, _| this.step_speed(d),
            ),
            self.stepper(
                "sub-size",
                "Subtitle size",
                format!("{:.0}%", status.subtitle_scale * 100.0),
                cx,
                |this, d, _| this.step_subtitle_scale(d as f64 * 0.1),
            ),
            self.stepper(
                "sub-delay",
                "Subtitle delay",
                format_delay(status.subtitle_delay),
                cx,
                |this, d, _| this.step_subtitle_delay(d as f64 * 0.1),
            ),
        ];
        let mut options = options;
        if let Some(quality) = self.quality {
            options.push(self.stepper(
                "quality",
                "Streaming quality",
                quality.label(),
                cx,
                |this, d, cx| this.step_quality(d, cx),
            ));
        }

        Self::panel_frame("tracks-panel")
            .flex()
            .gap_3()
            .child(Self::panel_column("Audio", audio))
            .child(Self::panel_column("Subtitles", subtitles))
            .child(Self::panel_column("Options", options))
            .into_any_element()
    }

    /// The card that panels sit in, just above the control bar.
    fn panel_frame(id: &'static str) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .absolute()
            .bottom(px(BAR_HEIGHT + 10.0))
            .right_0()
            .w(px(PANEL_WIDTH))
            .max_w_full()
            .p_3()
            .rounded_2xl()
            .bg(rgba(SURFACE))
            .border_1()
            .border_color(rgba(BORDER))
            .text_color(rgb(TEXT))
            // Clicks inside the panel must not close it or pause the video.
            // (No hover tracking: an open panel keeps the controls up anyway,
            // and a hover flag set here would stick once the panel closes.)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
    }

    /// A small 16:9 picture for an episode, with its progress along the bottom.
    fn episode_thumb(episode: &super::Episode, width: f32) -> AnyElement {
        let label = episode.number.map(|n| n.to_string()).unwrap_or_default();
        div()
            .relative()
            .flex_none()
            .w(px(width))
            .h(px(width * 9.0 / 16.0))
            .rounded_md()
            .overflow_hidden()
            .bg(rgb(0x2b3a55))
            .child(match &episode.thumb {
                Some(path) => img(path.clone())
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .into_any_element(),
                None => div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(MUTED))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(label)
                    .into_any_element(),
            })
            .children(episode.progress.filter(|_| !episode.watched).map(|p| {
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .h(px(3.0))
                    .bg(rgba(0x00000080))
                    .child(div().h_full().w(relative(p)).bg(rgb(ACCENT)))
            }))
            .into_any_element()
    }

    fn episodes_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let seasons = self.seasons();
        let season = self.episode_season;
        let current = self.current;

        let tabs: Vec<AnyElement> = seasons
            .iter()
            .map(|&s| {
                div()
                    .id(("season", s as usize))
                    .px_3()
                    .py_1()
                    .rounded_full()
                    .text_sm()
                    .cursor_pointer()
                    .when(s == season, |d| d.bg(rgba(0xffffff26)))
                    .hover(|d| d.bg(rgba(0xffffff1f)))
                    .child(season_name(s))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.episode_season = s;
                        this.poke(cx);
                    }))
                    .into_any_element()
            })
            .collect();

        let rows: Vec<AnyElement> = self
            .episodes
            .iter()
            .enumerate()
            .filter(|(_, e)| e.season == season)
            .map(|(index, episode)| {
                let playing = current == Some(index);
                let title = match episode.number {
                    Some(n) => format!("{n}. {}", episode.title),
                    None => episode.title.clone(),
                };
                let state = if playing {
                    "Now playing".to_string()
                } else if episode.watched {
                    "Watched".to_string()
                } else if let Some(p) = episode.progress {
                    format!("{:.0}% watched", p * 100.0)
                } else {
                    String::new()
                };
                let key = episode.key.clone();
                div()
                    .id(("episode", index))
                    .flex()
                    .items_center()
                    .gap_3()
                    .p_1p5()
                    .rounded_lg()
                    .cursor_pointer()
                    .when(playing, |d| d.bg(rgba(0x4c8dff2e)))
                    .hover(|d| d.bg(rgba(0xffffff14)))
                    .child(Self::episode_thumb(episode, 112.0))
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .child(div().text_sm().child(title))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(if playing { ACCENT } else { MUTED }))
                                    .child(state),
                            ),
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if playing {
                            this.panel = None;
                            this.poke(cx);
                        } else {
                            this.play_episode(key.clone(), cx);
                        }
                    }))
                    .into_any_element()
            })
            .collect();

        Self::panel_frame("episodes-panel")
            .flex()
            .flex_col()
            .gap_2()
            .when(seasons.len() > 1, |d| {
                d.child(div().flex().flex_wrap().gap_1().children(tabs))
            })
            .child(
                div()
                    .id("episode-list")
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .max_h(px(340.0))
                    .overflow_y_scroll()
                    .children(rows),
            )
            .into_any_element()
    }

    fn text_button(
        id: &'static str,
        label: &'static str,
        primary: bool,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        div()
            .id(id)
            .px_4()
            .py_1p5()
            .rounded_full()
            .text_sm()
            .font_weight(FontWeight::SEMIBOLD)
            .cursor_pointer()
            .bg(rgba(if primary { 0xf4f5f7ff } else { 0xffffff1f }))
            .text_color(rgb(if primary { 0x101218 } else { TEXT }))
            .hover(|s| s.opacity(0.85))
            .child(label)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    action(this, cx);
                }),
            )
            .into_any_element()
    }

    /// Shown when an episode ends: counts down to the next one.
    fn up_next_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let seconds = self.up_next_remaining()?;
        let next = self.next_episode()?;
        let heading = match next.number {
            Some(n) => format!("{} · Episode {n}", season_name(next.season)),
            None => season_name(next.season),
        };
        Some(
            div()
                .absolute()
                .right(px(24.0))
                .bottom(px(BAR_CLEARANCE))
                .w(px(380.0))
                .p_4()
                .rounded_2xl()
                .bg(rgba(SURFACE))
                .border_1()
                .border_color(rgba(BORDER))
                .text_color(rgb(TEXT))
                .flex()
                .flex_col()
                .gap_3()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(MUTED))
                        .child("UP NEXT"),
                )
                .child(
                    div()
                        .flex()
                        .gap_3()
                        .child(Self::episode_thumb(next, 128.0))
                        .child(
                            div()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap_0p5()
                                .child(div().text_xs().text_color(rgb(MUTED)).child(heading))
                                .child(
                                    div()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(next.title.clone()),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(rgb(MUTED))
                                        .child(format!("Playing in {seconds} s")),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(Self::text_button(
                            "play-now",
                            "Play now",
                            true,
                            cx,
                            |this, cx| this.play_next(cx),
                        ))
                        .child(if self.status.ended {
                            Self::text_button(
                                "up-next-back",
                                "Back to library",
                                false,
                                cx,
                                |this, cx| this.exit(cx),
                            )
                        } else {
                            Self::text_button(
                                "watch-credits",
                                "Watch credits",
                                false,
                                cx,
                                |this, cx| {
                                    this.watch_credits();
                                    this.poke(cx);
                                },
                            )
                        }),
                )
                .into_any_element(),
        )
    }

    /// Keyboard shortcuts, opened with F1 or `?`.
    fn shortcuts_overlay(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut rows: Vec<(&str, &str)> = vec![
            ("Space / K", "Play or pause"),
            ("← → / J L", "Back or forward 10 seconds"),
            ("↑ ↓", "Volume"),
            ("M", "Mute"),
            ("S", "Next subtitle track (or off)"),
            ("A", "Next audio track"),
            ("C", "Audio and subtitles panel"),
            ("[ ]", "Slower / faster"),
            ("Z X", "Subtitles earlier / later"),
            ("F / double-click", "Fullscreen"),
        ];
        if !self.episodes.is_empty() {
            rows.push(("N", "Next episode"));
            rows.push(("E", "Episodes"));
        }
        rows.push(("Enter", "Skip intro, or play the next episode now"));
        rows.push(("F1 / ?", "This list"));
        rows.push(("Esc", "Close, leave fullscreen, or go back"));

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .id("shortcuts")
                    .w(px(460.0))
                    .p_5()
                    .rounded_2xl()
                    .bg(rgba(0x15171df5))
                    .border_1()
                    .border_color(rgba(BORDER))
                    .text_color(rgb(TEXT))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .pb_1()
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Keyboard shortcuts"),
                            )
                            .child(self.icon_button(
                                "close-shortcuts",
                                Icon::Close,
                                18.0,
                                false,
                                cx,
                                |this, _, _| this.panel = None,
                            )),
                    )
                    .children(rows.into_iter().map(|(keys, action)| {
                        div()
                            .flex()
                            .items_center()
                            .gap_4()
                            .child(
                                div().w(px(150.0)).flex_none().child(
                                    div()
                                        .px_2()
                                        .py_0p5()
                                        .rounded_md()
                                        .bg(rgba(0xffffff1a))
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(keys),
                                ),
                            )
                            .child(div().text_sm().text_color(rgb(MUTED)).child(action))
                    })),
            )
            .into_any_element()
    }

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
                    .children(match self.panel {
                        Some(Panel::Tracks) => Some(self.track_panel(cx)),
                        Some(Panel::Episodes) => Some(self.episodes_panel(cx)),
                        Some(Panel::Shortcuts) | None => None,
                    })
                    .child(self.control_bar(window, cx)),
            )
            .into_any_element()
    }

    fn centered_message(top: f32, text: SharedString) -> AnyElement {
        div()
            .absolute()
            .top(px(top))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(chip().px_4().py_2().text_sm().child(text))
            .into_any_element()
    }
}

impl Render for Player {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let visible = self.controls_visible();
        self.place_subtitles(window, visible);
        if self
            .toast
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() > TOAST_DURATION)
        {
            self.toast = None;
        }

        // No background: transparent pixels are where the video shows through.
        let mut overlay = Vec::new();
        if visible {
            overlay.push(self.top_bar(cx));
            overlay.push(self.bottom(window, cx));
        }
        if self.status.buffering && !self.status.paused {
            let middle = f32::from(window.viewport_size().height) / 2.0 - 20.0;
            overlay.push(Self::centered_message(middle, "Loading…".into()));
        }
        if let Some((message, _)) = &self.toast {
            overlay.push(Self::centered_message(84.0, message.clone()));
        }
        if let Some(card) = self.up_next_card(cx) {
            overlay.push(card);
        } else if let Some((label, _)) = self.skip_target() {
            overlay.push(
                div()
                    .absolute()
                    .right(px(24.0))
                    .bottom(px(BAR_CLEARANCE))
                    .child(Self::text_button("skip", label, true, cx, |this, cx| {
                        this.skip_segment();
                        this.poke(cx);
                    }))
                    .into_any_element(),
            );
        }
        if self.panel == Some(Panel::Shortcuts) {
            overlay.push(self.shortcuts_overlay(cx));
        }

        div()
            .id("stage")
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .when(!visible, |d| d.cursor(CursorStyle::None))
            .on_key_down(cx.listener(Self::on_key))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_stage_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .children(overlay)
    }
}
