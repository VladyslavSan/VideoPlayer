//! Drawing the player overlay: a floating control bar, the audio/subtitle
//! panel above it, the title, and transient messages.

use std::time::Instant;

use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, Div, ElementId, FontWeight, MouseButton,
    MouseDownEvent, ObjectFit, SharedString, Window, canvas, div, img, prelude::*, px, relative,
    rgb, rgba, svg,
};

use super::{Panel, Player, TOAST_DURATION, segments, tracks};
use crate::jellyfin::{QUALITIES, Quality};
use crate::ui::icons::{Icon, icon};
use crate::ui::menu;
use crate::video::{SoundProfile, TrackKind};

const TEXT: u32 = 0xf4f5f7;
const MUTED: u32 = 0x9aa3b2;
const ACCENT: u32 = 0x4c8dff;
const SURFACE: u32 = 0x15171de8;
const BORDER: u32 = 0xffffff1a;
/// Intros, recaps and credits on the seek bar.
const SEGMENT: u32 = 0xffcf5c99;
/// The gaps that split the seek bar into chapters.
const CHAPTER_GAP: u32 = 0x000000b3;
/// The hover preview's picture.
const PREVIEW_WIDTH: f32 = 240.0;
const PREVIEW_HEIGHT: f32 = 135.0;
/// More chapters than this are usually fixed-interval markers: no ticks.
const MAX_CHAPTER_TICKS: usize = 40;

const BAR_HEIGHT: f32 = 52.0;
const BAR_BOTTOM: f32 = 24.0;
const BAR_MAX_WIDTH: f32 = 860.0;
const PANEL_WIDTH: f32 = 720.0;
/// Room the speed chip ("1.75×") takes in the control bar.
const SPEED_CHIP_WIDTH: f32 = 60.0;
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
    /// A round icon button. `active` (0 to 1) lights it up, for a button
    /// whose panel is open, following that panel's fade; a changed `glyph`
    /// dissolves into the new one.
    fn icon_button(
        &self,
        id: impl Into<ElementId>,
        glyph: Icon,
        size: f32,
        active: f32,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        let id = id.into();
        let glyph = self
            .swaps
            .draw(format!("icon-{id:?}"), glyph.path(), move |path| {
                svg().path(path).size(px(size)).text_color(rgb(TEXT))
            });
        div()
            .id(id)
            .size(px(40.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .cursor_pointer()
            .when(active > 0.0, |d| {
                d.bg(rgba(0xffffff00 | (0x26 as f32 * active).round() as u32))
            })
            .hover(|s| s.bg(rgba(0xffffff1f)))
            .child(glyph)
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
        self.icon_button(id, glyph, 26.0, 0.0, cx, move |this, _, _| {
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
                0.0,
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
        // The time preview fades in under the mouse, and the bar thickens
        // with it.
        let hover = self.fades.preview.value();
        let track_height = 4.0 + 2.0 * hover;
        let bounds = self.seek_bounds.clone();
        let bands: Vec<(f32, f32)> = self
            .segments
            .iter()
            .map(|s| (fraction(s.start), fraction(s.end)))
            .filter(|(start, end)| end > start)
            .collect();
        let ticks: Vec<f32> = if (2..=MAX_CHAPTER_TICKS).contains(&self.chapters.len()) {
            self.chapters
                .iter()
                .map(|(_, start)| fraction(*start))
                .filter(|f| *f > 0.002 && *f < 0.998)
                .collect()
        } else {
            Vec::new()
        };

        let preview = self.fades.preview.on_screen().then(|| {
            let x = self.preview_x;
            let area = self.seek_bounds.get();
            let time = super::fraction_at(area, x) * duration;
            let offset = f32::from(x - area.origin.x).clamp(0.0, f32::from(area.size.width));
            let chapter = segments::chapter_at(&self.chapters, time)
                .and_then(|(title, _)| segments::chapter_label(title));
            let label = match chapter {
                Some(chapter) => format!("{} · {chapter}", super::format_time(time)),
                None => super::format_time(time),
            };
            let picture = self.previews.as_ref().map(|previews| {
                // Black until the first picture arrives.
                div()
                    .w(px(PREVIEW_WIDTH))
                    .h(px(PREVIEW_HEIGHT))
                    .rounded_lg()
                    .overflow_hidden()
                    .bg(rgb(0x000000))
                    .border_1()
                    .border_color(rgba(BORDER))
                    .children(
                        previews
                            .frame_for(time)
                            .map(|path| img(path).size_full().object_fit(ObjectFit::Contain)),
                    )
            });
            div()
                .absolute()
                .opacity(hover)
                .bottom(px(SLIDER_HEIGHT + 6.0))
                .left(px(offset - 160.0))
                .w(px(320.0))
                .flex()
                .flex_col()
                .items_center()
                .gap_1p5()
                .children(picture)
                .child(
                    chip()
                        .max_w(px(320.0))
                        .px_2()
                        .py_0p5()
                        .text_xs()
                        .truncate()
                        .child(label),
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
                    )
                    .children(bands.into_iter().map(|(start, end)| {
                        div()
                            .absolute()
                            .top_0()
                            .h_full()
                            .left(relative(start))
                            .w(relative(end - start))
                            .bg(rgba(SEGMENT))
                    }))
                    .children(ticks.into_iter().map(|at| {
                        div()
                            .absolute()
                            .top_0()
                            .h_full()
                            .left(relative(at))
                            .ml(px(-1.0))
                            .w(px(2.0))
                            .bg(rgba(CHAPTER_GAP))
                    })),
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
        let speed = self.fades.speed.value();

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
                self.icon_button("play", play_icon, 24.0, 0.0, cx, |this, _, _| {
                    this.toggle_pause()
                }),
            )
            .child(self.skip_button("skip-back", Icon::SkipBack, -self.controls.back_seconds, cx))
            .child(self.skip_button(
                "skip-forward",
                Icon::SkipForward,
                self.controls.forward_seconds,
                cx,
            ))
            .when(self.next_episode().is_some(), |bar| {
                bar.child(self.icon_button(
                    "next-episode",
                    Icon::NextEpisode,
                    20.0,
                    0.0,
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
                self.icon_button("mute", volume_icon, 22.0, 0.0, cx, |this, _, _| {
                    this.toggle_mute()
                }),
            )
            .child(self.volume_slider(cx))
            .child(div().w(px(6.0)).flex_none())
            .when(self.fades.speed.on_screen(), |bar| {
                // Opens up its room as it fades in, rather than shoving the
                // seek bar aside.
                bar.child(
                    div()
                        .flex_none()
                        .w(px(SPEED_CHIP_WIDTH * speed))
                        .overflow_hidden()
                        .opacity(speed)
                        .flex()
                        .justify_center()
                        .child(self.speed_chip(cx)),
                )
            })
            .when(!self.episodes.is_empty(), |bar| {
                bar.child(self.icon_button(
                    "episodes",
                    Icon::Episodes,
                    21.0,
                    self.fades.panels[Panel::Episodes.index()].value(),
                    cx,
                    |this, _, _| this.toggle_panel(Panel::Episodes),
                ))
            })
            .child(self.icon_button(
                "tracks",
                Icon::Subtitles,
                22.0,
                self.fades.panels[Panel::Tracks.index()].value(),
                cx,
                |this, _, _| this.toggle_panel(Panel::Tracks),
            ))
            .child(self.icon_button(
                "fullscreen",
                fullscreen_icon,
                20.0,
                0.0,
                cx,
                |_, window, _| window.toggle_fullscreen(),
            ))
            .into_any_element()
    }

    /// The playback speed, when it isn't normal; opens the track panel.
    fn speed_chip(&self, cx: &mut Context<Self>) -> AnyElement {
        // While it fades out at normal speed again, it keeps its last value.
        let speed = self.speed_shown;
        div()
            .id("speed")
            .flex_none()
            .px_2()
            .py_1()
            .rounded_full()
            .text_sm()
            .whitespace_nowrap()
            .cursor_pointer()
            .hover(|s| s.bg(rgba(0xffffff1f)))
            .child(
                self.swaps
                    .draw("speed", format_speed(speed), |text| div().child(text)),
            )
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.toggle_panel(Panel::Tracks);
                this.poke(cx);
            }))
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
        &self,
        id: ElementId,
        primary: SharedString,
        secondary: SharedString,
        selected: bool,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        let check = format!("check-{id:?}");
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
                    .child(
                        self.swaps
                            .draw(check, if selected { "on" } else { "off" }, |state| {
                                div().size(px(16.0)).when(state == "on", |d| {
                                    d.child(
                                        icon(Icon::Check).size(px(16.0)).text_color(rgb(ACCENT)),
                                    )
                                })
                            }),
                    ),
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
                        0.0,
                        cx,
                        move |this, _, cx| down(this, -1, cx),
                    )))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .justify_center()
                            .text_sm()
                            .child(
                                self.swaps
                                    .draw(format!("{id}-value"), value, |value| div().child(value)),
                            ),
                    )
                    .child(small(self.icon_button(
                        (id, 1usize),
                        Icon::Plus,
                        16.0,
                        0.0,
                        cx,
                        move |this, _, cx| step(this, 1, cx),
                    ))),
            )
            .into_any_element()
    }

    /// "Streaming quality" with a button that opens a menu of the presets.
    fn quality_picker(
        &self,
        current: Quality,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lit = self.quality_menu.lit(|_| true);
        let mut picker = div().relative().flex().child(
            menu::trigger(
                "quality",
                self.swaps
                    .draw("quality", current.label(), |label| div().child(label)),
                lit,
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                if !this.quality_menu.close() {
                    this.quality_menu.open(current);
                }
                this.poke(cx);
            })),
        );
        for shown in self.quality_menu.drawn() {
            let items = QUALITIES
                .iter()
                .enumerate()
                .map(|(i, &quality)| {
                    menu::item(
                        ("quality-choice", i),
                        quality.menu_label(),
                        quality == *shown.value,
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.choose_quality(quality, cx);
                        this.poke(cx);
                    }))
                    .into_any_element()
                })
                .collect();
            picker = picker.child(menu::popup(
                &shown,
                true,
                items,
                window,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.quality_menu.close();
                    this.poke(cx);
                }),
            ));
        }
        Self::menu_option("Streaming quality", picker)
    }

    /// "Sound filter" with a button that opens a menu: off, the presets
    /// and the custom tunings.
    fn sound_picker(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let current = self.sound_choice();
        let label = current.map_or_else(|| "Off".to_string(), |profile| profile.label());
        let lit = self.sound_menu.lit(|_| true);
        let mut picker = div().relative().flex().child(
            menu::trigger(
                "sound",
                self.swaps.draw("sound", label, |label| div().child(label)),
                lit,
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                if !this.sound_menu.close() {
                    this.sound_menu.open(current);
                }
                this.poke(cx);
            })),
        );
        for shown in self.sound_menu.drawn() {
            let choices = std::iter::once(None).chain(SoundProfile::ALL.map(Some));
            let items = choices
                .enumerate()
                .map(|(i, choice)| {
                    let label = choice.map_or_else(|| "Off".to_string(), |p| p.label());
                    menu::item(("sound-choice", i), label, choice == *shown.value)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.choose_sound(choice, cx);
                            this.poke(cx);
                        }))
                        .into_any_element()
                })
                .collect();
            picker = picker.child(menu::popup(
                &shown,
                true,
                items,
                window,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.sound_menu.close();
                    this.poke(cx);
                }),
            ));
        }
        Self::menu_option("Sound filter", picker)
    }

    /// An option in the tracks panel that's picked from a menu: its name
    /// over the button.
    fn menu_option(title: &'static str, picker: Div) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .px_2()
            .py_1()
            .child(div().text_xs().text_color(rgb(MUTED)).child(title))
            .child(picker)
            .into_any_element()
    }

    fn track_panel(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let mut audio = Vec::new();
        for (i, track) in self.tracks_of(TrackKind::Audio).enumerate() {
            let label = tracks::label(track, i + 1);
            let id = track.id;
            audio.push(self.choice_row(
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
        let mut subtitles = vec![self.choice_row(
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
            subtitles.push(self.choice_row(
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
                |this, d, cx| this.step_subtitle_scale(d as f64 * 0.1, cx),
            ),
            self.stepper(
                "sub-delay",
                "Subtitle delay",
                format_delay(status.subtitle_delay),
                cx,
                |this, d, cx| this.step_subtitle_delay(d as f64 * 0.1, cx),
            ),
        ];
        let mut options = options;
        options.push(self.sound_picker(window, cx));
        if let Some(quality) = self.quality {
            options.push(self.quality_picker(quality, window, cx));
        }

        Self::panel_frame("tracks-panel")
            .flex()
            .gap_3()
            .child(Self::panel_column("Audio", audio))
            .child(Self::panel_column("Subtitles", subtitles))
            .child(Self::panel_column("Options", options))
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

    fn episodes_panel(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let seasons = self.seasons();
        let season = self.episode_season;
        // Switching seasons: the old list dissolves into the new one, and
        // the tab's highlight moves across.
        let switch = self.season_switch.and_then(|(from, at)| {
            let t = at.elapsed().as_secs_f32() / crate::motion::OVERLAY.time().as_secs_f32();
            (crate::motion::enabled() && t < 1.0 && from != season)
                .then(|| (from, crate::motion::ease_in_out(t)))
        });
        let lit = |s: u16| match switch {
            _ if s == season => switch.map_or(1.0, |(_, t)| t),
            Some((from, t)) if s == from => 1.0 - t,
            _ => 0.0,
        };

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
                    .when(lit(s) > 0.0, |d| {
                        d.bg(rgba(0xffffff00 | (0x26 as f32 * lit(s)).round() as u32))
                    })
                    .hover(|d| d.bg(rgba(0xffffff1f)))
                    .child(season_name(s))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if this.episode_season != s {
                            this.season_switch = Some((this.episode_season, Instant::now()));
                            this.episode_season = s;
                        }
                        this.poke(cx);
                    }))
                    .into_any_element()
            })
            .collect();

        let mut list = |season: u16| self.episode_rows(season, cx);
        let shown = div()
            .id(("episode-list", season as usize))
            .flex()
            .flex_col()
            .gap_0p5()
            .max_h(px(340.0))
            .overflow_y_scroll()
            .children(list(season));
        let shown = match switch {
            Some((from, t)) => div()
                .relative()
                .child(shown.opacity(t))
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .max_h(px(340.0))
                        .overflow_hidden()
                        .opacity(1.0 - t)
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .children(list(from)),
                )
                .into_any_element(),
            None => shown.into_any_element(),
        };

        Self::panel_frame("episodes-panel")
            .flex()
            .flex_col()
            .gap_2()
            .when(seasons.len() > 1, |d| {
                d.child(div().flex().flex_wrap().gap_1().children(tabs))
            })
            .child(shown)
    }

    /// The episodes of `season`, for the episodes panel.
    fn episode_rows(&self, season: u16, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let current = self.current;
        self.episodes
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
            .collect()
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

    /// Shown when an episode ends: counts down to the next one. `ended`:
    /// the video is over (rather than in its credits).
    fn up_next_card(
        &self,
        seconds: u64,
        ended: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
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
                        .child(if ended {
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
        use super::controls::{Action, key_label};
        // As set in Settings → Controls.
        let mut rows: Vec<(String, String)> = Vec::new();
        for action in Action::ALL {
            if matches!(action, Action::NextEpisode | Action::EpisodesPanel)
                && self.episodes.is_empty()
            {
                continue;
            }
            let keys = self.controls.keys(action);
            if keys.is_empty() {
                continue;
            }
            let keys: Vec<String> = keys.iter().map(|k| key_label(k)).collect();
            let label = match action {
                Action::Back => format!("Back {:.0} seconds", self.controls.back_seconds),
                Action::Forward => {
                    format!("Forward {:.0} seconds", self.controls.forward_seconds)
                }
                other => other.label().to_string(),
            };
            rows.push((keys.join(" / "), label));
        }
        rows.push(("Double-click".into(), "Fullscreen".into()));
        rows.push(("Click".into(), "Show or hide the controls".into()));
        rows.push(("?".into(), "This list of keys".into()));
        rows.push(("Esc".into(), "Close, leave fullscreen, or go back".into()));

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .id("shortcuts")
                    // Two columns: the list is long once every action has
                    // its keys.
                    .w(px(980.0))
                    .max_w(relative(0.94))
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
                                0.0,
                                cx,
                                |this, _, _| this.panel = None,
                            )),
                    )
                    .child({
                        let half = rows.len().div_ceil(2);
                        let mut rows = rows.into_iter();
                        let column = |rows: Vec<(String, String)>| {
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .flex_col()
                                .gap_2()
                                .children(rows.into_iter().map(|(keys, action)| {
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_3()
                                        .child(
                                            div().w(px(130.0)).flex_none().child(
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
                                        .child(
                                            div()
                                                .min_w(px(0.0))
                                                .text_sm()
                                                .text_color(rgb(MUTED))
                                                .child(action),
                                        )
                                }))
                        };
                        let left: Vec<_> = rows.by_ref().take(half).collect();
                        let right: Vec<_> = rows.collect();
                        div()
                            .flex()
                            .gap_6()
                            .child(column(left))
                            .child(column(right))
                    }),
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
                    // Both can be on screen for a moment, one fading into the
                    // other.
                    .when(self.panel_drawn(Panel::Tracks), |d| {
                        let shown = self.fades.panels[Panel::Tracks.index()].value();
                        d.child(self.track_panel(window, cx).opacity(shown))
                    })
                    .when(self.panel_drawn(Panel::Episodes), |d| {
                        let shown = self.fades.panels[Panel::Episodes.index()].value();
                        d.child(self.episodes_panel(cx).opacity(shown))
                    })
                    .child(self.control_bar(window, cx)),
            )
            .into_any_element()
    }

    fn panel_drawn(&self, panel: Panel) -> bool {
        self.fades.panels[panel.index()].on_screen()
    }

    fn centered_message(top: f32, text: SharedString, shown: f32) -> AnyElement {
        div()
            .absolute()
            .opacity(shown)
            .top(px(top))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(chip().px_4().py_2().text_sm().child(text))
            .into_any_element()
    }
}

impl Player {
    /// Points each fade at where it should be now. Everything on the
    /// player that comes and goes does so through these.
    fn update_fades(&mut self, window: &Window) {
        let visible = self.controls_visible();
        self.fades.controls.set(visible);
        for panel in Panel::ALL {
            self.fades.panels[panel.index()].set(self.panel == Some(panel));
        }
        if self
            .toast_at
            .is_some_and(|at| at.elapsed() > TOAST_DURATION)
        {
            self.toast_at = None;
            self.toasts.show(None);
        }
        // "Loading…" only once the wait is long enough to notice, so a
        // quick seek never flashes it.
        let loading = self.status.buffering && !self.status.paused && self.stall.is_none();
        if !loading {
            self.loading_since = None;
        } else if self.loading_since.is_none() {
            self.loading_since = Some(Instant::now());
        }
        let notice = if self.stall.is_some() {
            Some("Lost the connection to the server. Reconnecting…")
        } else {
            self.loading_since
                .filter(|since| since.elapsed() >= crate::motion::WAIT.time())
                .map(|_| "Loading…")
        };
        self.notice.show(notice.map(SharedString::from));
        let up_next = self
            .up_next_remaining()
            .filter(|_| self.next_episode().is_some());
        if let Some(seconds) = up_next {
            self.up_next_shown = (seconds, self.status.ended);
        }
        self.fades.up_next.set(up_next.is_some());
        let skip = self.skip_target().filter(|_| up_next.is_none());
        if let Some((label, _)) = skip {
            self.skip_shown = label;
        }
        self.fades.skip.set(skip.is_some());
        let preview = self.seek_hover.filter(|_| self.status.duration > 0.0);
        if let Some(x) = preview {
            self.preview_x = x;
        }
        self.fades.preview.set(preview.is_some());
        let speed = (self.status.speed - 1.0).abs() > 0.01;
        if speed {
            self.speed_shown = self.status.speed;
        }
        self.fades.speed.set(speed);
        self.fades.video.set(self.status.video_shown);
        // Subtitles move out of the bar's way as it fades in.
        self.place_subtitles(window, self.fades.controls.value());
    }

    /// Whether anything is still on its way in or out, to keep drawing.
    fn fading(&self) -> bool {
        self.fades.moving()
            || self.toasts.moving()
            || self.notice.moving()
            || self
                .season_switch
                .is_some_and(|(_, at)| at.elapsed() < crate::motion::OVERLAY.time())
    }
}

impl Render for Player {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let visible = self.controls_visible();
        // The quality and sound menus live in the tracks panel and close
        // with it.
        if self.panel != Some(Panel::Tracks) {
            self.quality_menu.close();
            self.sound_menu.close();
        }
        self.update_fades(window);

        // No background: transparent pixels are where the video shows through.
        let mut overlay = Vec::new();
        if self.fades.controls.on_screen() {
            let shown = self.fades.controls.value();
            overlay.push(
                div()
                    .absolute()
                    .inset_0()
                    .opacity(shown)
                    .child(self.top_bar(cx))
                    .child(self.bottom(window, cx))
                    .into_any_element(),
            );
        }
        let middle = f32::from(window.viewport_size().height) / 2.0 - 20.0;
        for (text, shown) in self.notice.drawn() {
            overlay.push(Self::centered_message(middle, text, shown));
        }
        for (text, shown) in self.toasts.drawn() {
            overlay.push(Self::centered_message(84.0, text, shown));
        }
        if self.fades.skip.on_screen() {
            let label = self.skip_shown;
            overlay.push(
                div()
                    .absolute()
                    .right(px(24.0))
                    .bottom(px(BAR_CLEARANCE))
                    .opacity(self.fades.skip.value())
                    .child(Self::text_button("skip", label, true, cx, |this, cx| {
                        this.skip_segment();
                        this.poke(cx);
                    }))
                    .into_any_element(),
            );
        }
        if self.fades.up_next.on_screen() {
            let (seconds, ended) = self.up_next_shown;
            if let Some(card) = self.up_next_card(seconds, ended, cx) {
                overlay.push(
                    div()
                        .absolute()
                        .inset_0()
                        .opacity(self.fades.up_next.value())
                        .child(card)
                        .into_any_element(),
                );
            }
        }
        if self.panel_drawn(Panel::Shortcuts) {
            let shown = self.fades.panels[Panel::Shortcuts.index()].value();
            overlay.push(
                div()
                    .absolute()
                    .inset_0()
                    .opacity(shown)
                    .child(self.shortcuts_overlay(cx))
                    .into_any_element(),
            );
        }
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
            .track_focus(&self.focus)
            .when(!visible, |d| d.cursor(CursorStyle::None))
            .on_key_down(cx.listener(Self::on_key))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_stage_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .children(overlay)
    }
}
