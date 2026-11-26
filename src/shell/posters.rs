//! How bright posters are: a little dimmed while browsing, lit up under
//! the mouse (or the arrow keys' cursor), easing between the two. Both
//! levels are settings (Settings > Appearance).

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Instant;

use gpui::{
    AnyElement, Bounds, Context, FontWeight, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels,
    SharedString, Window, canvas, div, prelude::*, px, rgb, rgba,
};

use super::{ACCENT, MUTED, Shell};
use crate::library::Db;

const REST_SETTING: &str = "poster_brightness";
const LIT_SETTING: &str = "poster_hover_brightness";
/// Brightness at rest and lit, as fractions of the picture's own.
const DEFAULT_REST: f32 = 0.8;
const DEFAULT_LIT: f32 = 1.0;
/// The sliders' range.
pub(super) const DIMMEST: f32 = 0.4;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Light {
    pub rest: f32,
    pub lit: f32,
}

impl Light {
    pub(super) fn load(db: &Db) -> Self {
        let read = |key: &str, default: f32| {
            db.setting(key)
                .ok()
                .flatten()
                .and_then(|v| v.parse::<u32>().ok())
                .map_or(default, |percent| percent as f32 / 100.0)
                .clamp(DIMMEST, 1.0)
        };
        Self {
            rest: read(REST_SETTING, DEFAULT_REST),
            lit: read(LIT_SETTING, DEFAULT_LIT),
        }
    }

    /// How bright a card is, `lit` (0..1) of the way to lit.
    pub(super) fn at(self, lit: f32) -> f32 {
        self.rest + (self.lit - self.rest) * lit
    }
}

/// The card under the mouse, and those easing back after it left.
#[derive(Default)]
pub(super) struct Hover {
    /// Card, since when, and how lit it was then.
    on: Option<(SharedString, Instant, f32)>,
    /// Card, since when, and how lit it was then.
    off: Vec<(SharedString, Instant, f32)>,
}

fn ease(started: Instant) -> f32 {
    if !crate::motion::enabled() {
        return 1.0;
    }
    let time = crate::motion::HOVER.time().as_secs_f32();
    crate::motion::ease_in_out((started.elapsed().as_secs_f32() / time).min(1.0))
}

impl Hover {
    /// How lit the card is from the mouse, 0..1.
    pub(super) fn level(&self, id: &SharedString) -> f32 {
        match &self.on {
            Some((on, at, from)) if on == id => from + (1.0 - from) * ease(*at),
            _ => self
                .off
                .iter()
                .find(|(off, ..)| off == id)
                .map_or(0.0, |(_, at, from)| from * (1.0 - ease(*at))),
        }
    }

    pub(super) fn set(&mut self, id: &SharedString, hovered: bool) {
        if hovered {
            if self.on.as_ref().is_some_and(|(on, ..)| on == id) {
                return;
            }
            // Each picks up from wherever it was.
            let from = self.level(id);
            self.off.retain(|(off, ..)| off != id);
            if let Some((on, at, was)) = self.on.take() {
                self.off
                    .push((on, Instant::now(), was + (1.0 - was) * ease(at)));
            }
            self.on = Some((id.clone(), Instant::now(), from));
        } else if self.on.as_ref().is_some_and(|(on, ..)| on == id) {
            let level = self.level(id);
            self.on = None;
            self.off.push((id.clone(), Instant::now(), level));
        }
    }

    /// Whether anything is still easing (and needs another frame); drops
    /// what has finished.
    pub(super) fn animating(&mut self) -> bool {
        self.off.retain(|(_, at, _)| ease(*at) < 1.0);
        !self.off.is_empty() || self.on.as_ref().is_some_and(|(_, at, _)| ease(*at) < 1.0)
    }
}

/// Where each slider's track was drawn, for turning the mouse into a value.
pub(super) type Tracks = RefCell<HashMap<&'static str, Bounds<Pixels>>>;

/// What a slider runs over: `min` to `max` in steps of `step`.
#[derive(Clone, Copy)]
pub(super) struct Range {
    pub min: f32,
    pub max: f32,
    pub step: f32,
}

impl Range {
    /// Poster brightness, as a fraction.
    const BRIGHTNESS: Range = Range {
        min: DIMMEST,
        max: 1.0,
        step: 0.01,
    };

    pub(super) fn fraction(self, value: f32) -> f32 {
        ((value - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
    }

    pub(super) fn at(self, fraction: f32) -> f32 {
        let value = self.min + fraction.clamp(0.0, 1.0) * (self.max - self.min);
        ((value / self.step).round() * self.step).clamp(self.min, self.max)
    }
}

fn percent(value: f32) -> String {
    format!("{:.0}%", value * 100.0)
}

/// The darkening laid over a card's picture to make it `brightness`.
pub(super) fn shade(brightness: f32) -> AnyElement {
    let alpha = ((1.0 - brightness).clamp(0.0, 1.0) * 255.0).round() as u32;
    div()
        .absolute()
        .inset_0()
        .bg(rgba(alpha))
        .into_any_element()
}

impl Shell {
    pub(super) fn set_poster_light(&mut self, light: Light) {
        let light = Light {
            rest: light.rest.clamp(DIMMEST, 1.0),
            lit: light.lit.clamp(DIMMEST, 1.0),
        };
        let old = self.poster_light.get();
        self.poster_light.set(light);
        for (key, new, old) in [
            (REST_SETTING, light.rest, old.rest),
            (LIT_SETTING, light.lit, old.lit),
        ] {
            let percent = (new * 100.0).round() as u32;
            if percent != (old * 100.0).round() as u32
                && let Err(e) = self.db.set_setting(key, &percent.to_string())
            {
                eprintln!("[settings] saving {key}: {e:#}");
            }
        }
    }

    /// A setting picked along a track: the text on the left, the track and
    /// the value (written by `show`) on the right. Click or drag anywhere on
    /// the row.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn slider_row(
        &self,
        id: &'static str,
        title: impl Into<SharedString>,
        description: impl Into<SharedString>,
        value: f32,
        range: Range,
        show: fn(f32) -> String,
        cx: &mut Context<Self>,
        set: impl Fn(&mut Shell, f32) + 'static,
    ) -> AnyElement {
        let fraction = range.fraction(value);
        let set = std::rc::Rc::new(set);
        let pick = std::rc::Rc::new(move |this: &mut Shell, x: Pixels| {
            let Some(track) = this.slider_tracks.borrow().get(id).copied() else {
                return;
            };
            set(this, range.at((x - track.left()) / track.size.width));
        });
        let pick_on_move = pick.clone();
        const TRACK: f32 = 220.0;
        const KNOB: f32 = 16.0;
        let tracks = self.slider_tracks.clone();
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
            .child(
                // Takes the room the track leaves, so a long description
                // wraps (and the row grows) instead of being measured as
                // one line and spilling over what follows.
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .gap_1()
                    .min_w(px(0.0))
                    .child(div().font_weight(FontWeight::MEDIUM).child(title.into()))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(MUTED))
                            .child(description.into()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .relative()
                            .w(px(TRACK))
                            .h(px(KNOB))
                            .cursor_pointer()
                            .child(
                                canvas(
                                    move |b, _, _| {
                                        tracks.borrow_mut().insert(id, b);
                                    },
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .inset_0(),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .right_0()
                                    .top(px(KNOB / 2.0 - 2.0))
                                    .h(px(4.0))
                                    .rounded_full()
                                    .bg(rgba(0xffffff2e)),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .top(px(KNOB / 2.0 - 2.0))
                                    .w(px(TRACK * fraction))
                                    .h(px(4.0))
                                    .rounded_full()
                                    .bg(rgb(ACCENT)),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .left(px((TRACK - KNOB) * fraction))
                                    .size(px(KNOB))
                                    .rounded_full()
                                    .bg(rgb(0xffffff)),
                            ),
                    )
                    .child(
                        div()
                            .w(px(64.0))
                            .text_sm()
                            .text_color(rgb(MUTED))
                            .child(show(value)),
                    ),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.slider_drag = Some(id);
                    pick(this, event.position.x);
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                if this.slider_drag == Some(id) && event.pressed_button == Some(MouseButton::Left) {
                    pick_on_move(this, event.position.x);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.slider_drag = None),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.slider_drag = None),
            )
            .into_any_element()
    }

    /// Settings > Appearance.
    pub(super) fn appearance_settings(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let light = self.poster_light.get();
        // What the two look like, side by side.
        let sample = |label: &'static str, brightness: f32| {
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .relative()
                        .w(px(192.0))
                        .h(px(108.0))
                        .rounded_lg()
                        .overflow_hidden()
                        .bg(gpui::linear_gradient(
                            135.0,
                            gpui::linear_color_stop(rgb(0xe8604c), 0.0),
                            gpui::linear_color_stop(rgb(0x3b6fd8), 1.0),
                        ))
                        .child(shade(brightness)),
                )
                .child(div().text_sm().text_color(rgb(MUTED)).child(label))
        };
        let body = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.slider_row(
                "setting-poster-rest",
                "Brightness",
                "How bright posters and pictures are while you browse.",
                light.rest,
                Range::BRIGHTNESS,
                percent,
                cx,
                |this, value| {
                    let light = this.poster_light.get();
                    this.set_poster_light(Light {
                        rest: value,
                        ..light
                    });
                },
            ))
            .child(self.slider_row(
                "setting-poster-lit",
                "Under the mouse",
                "How bright the one under the mouse, or picked with the arrow keys, gets.",
                light.lit,
                Range::BRIGHTNESS,
                percent,
                cx,
                |this, value| {
                    let light = this.poster_light.get();
                    this.set_poster_light(Light {
                        lit: value,
                        ..light
                    });
                },
            ))
            .child(
                div()
                    .flex()
                    .gap_5()
                    .pt_3()
                    .px_4()
                    .child(sample("While browsing", light.rest))
                    .child(sample("Under the mouse", light.lit)),
            );
        vec![Self::section("Posters", body)]
    }
}

/// Needs a frame to draw the next step of a card easing.
pub(super) fn keep_animating(shell: &Shell, window: &mut Window) {
    if shell.card_hover.borrow_mut().animating() {
        window.request_animation_frame();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brightness_runs_from_rest_to_lit() {
        let light = Light {
            rest: 0.8,
            lit: 1.0,
        };
        assert_eq!(light.at(0.0), 0.8);
        assert_eq!(light.at(1.0), 1.0);
        assert!((light.at(0.5) - 0.9).abs() < 1e-6);
    }

    #[test]
    fn sliders_snap_to_steps() {
        let range = Range {
            min: 40.0,
            max: 1500.0,
            step: 10.0,
        };
        assert_eq!(range.at(0.0), 40.0);
        assert_eq!(range.at(1.0), 1500.0);
        assert_eq!(range.at(0.1234) % 10.0, 0.0);
        assert!((range.fraction(range.at(0.5)) - 0.5).abs() < 0.01);
    }

    #[test]
    fn hover_lights_and_dims() {
        let mut hover = Hover::default();
        let a: SharedString = "a".into();
        let b: SharedString = "b".into();
        assert_eq!(hover.level(&a), 0.0);
        hover.set(&a, true);
        hover.set(&b, true);
        // `a` is easing back, `b` lighting up.
        assert!(hover.off.iter().any(|(id, ..)| id == &a));
        assert!(hover.on.as_ref().is_some_and(|(id, ..)| id == &b));
        hover.set(&b, false);
        assert!(hover.on.is_none());
    }
}
