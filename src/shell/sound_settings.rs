//! Settings > Playback > Sound > Sound filters: one entry that unfolds into
//! the presets, the two custom tunings and a compact grid of knobs, each
//! explained by a tooltip. The filter is switched on and off (and can be
//! picked too) in the player; this is how each one sounds.
//!
//! Nothing here pops: the entry folds open and shut, the picked choice's
//! highlight fades, knobs glide to the values of a newly picked choice,
//! and tooltips wait a moment, then fade in (and out).

use std::f32::consts::PI;

use gpui::{
    Animation, AnimationExt, AnyElement, ClickEvent, Context, Div, MouseButton, MouseDownEvent,
    MouseMoveEvent, SharedString, Stateful, Transformation, anchored, canvas, deferred, div,
    prelude::*, px, radians, relative, rgb, rgba,
};

use super::arriving::{TIP_DELAY, Tip, tip_shown};
use super::posters::Range;
use super::{ACCENT, MUTED, Shell};
use crate::motion::{APPEAR, FOLD, SWITCH, ease_in_out};
use crate::ui::icons::{Icon, icon};
use crate::video::{Downmix, SoundPreset, SoundProfile, SoundSettings, SoundTuning};

const ENTRY: &str = "sound-entry";
/// Fades between the hint and the reset button, as the tuning in use
/// becomes changed or not.
const FOOTER: &str = "sound-footer";
const LEVELLER: &str = "sound-leveller";
const KNOB: f32 = 14.0;

/// A knob: its id, name, tooltip, range, how its value is shown, and the
/// part of the tuning it reads and sets.
struct Knob {
    id: &'static str,
    name: &'static str,
    tip: &'static str,
    range: Range,
    show: fn(f32) -> String,
    get: fn(&SoundTuning) -> f32,
    set: fn(&mut SoundTuning, f32),
}

const fn range(min: f32, max: f32, step: f32) -> Range {
    Range { min, max, step }
}

fn db(v: f32) -> String {
    if v.fract() == 0.0 {
        format!("{v:.0} dB")
    } else {
        format!("{v:.1} dB")
    }
}

fn ms(v: f32) -> String {
    format!("{v:.0} ms")
}

fn ratio(v: f32) -> String {
    if v <= 1.0 {
        "Off".to_string()
    } else if v.fract() == 0.0 {
        format!("{v:.0}:1")
    } else {
        format!("{v}:1")
    }
}

const KNOBS: [Knob; 9] = [
    Knob {
        id: "sound-cut-above",
        name: "Turn down above",
        tip: "Sound louder than this is turned down. Lower catches more of the sound; too low \
              flattens everything.",
        range: range(-40.0, -6.0, 1.0),
        show: db,
        get: |t| t.cut_above,
        set: |t, v| t.cut_above = v,
    },
    Knob {
        id: "sound-cut-ratio",
        name: "Turn down by",
        tip: "How hard loud sound is turned down: at 4:1, 8 dB too loud comes out 2 dB too \
              loud. Off leaves loud sound alone.",
        range: range(1.0, 10.0, 0.5),
        show: ratio,
        get: |t| t.cut_ratio,
        set: |t, v| t.cut_ratio = v,
    },
    Knob {
        id: "sound-attack",
        name: "Reaction time",
        tip: "How fast a loud sound is caught. Faster is safer; slower keeps hits punchy.",
        range: range(1.0, 100.0, 1.0),
        show: ms,
        get: |t| t.attack,
        set: |t, v| t.attack = v,
    },
    Knob {
        id: "sound-release",
        name: "Recovery time",
        tip: "How quickly it lets go after a loud sound. Too fast pumps; too slow keeps what \
              follows quiet.",
        range: range(50.0, 1500.0, 10.0),
        show: ms,
        get: |t| t.release,
        set: |t, v| t.release = v,
    },
    Knob {
        id: "sound-knee",
        name: "Softness",
        tip: "How gradually turning down (and lifting) starts around its level.",
        range: range(1.0, 8.0, 0.5),
        show: db,
        get: |t| t.knee,
        set: |t, v| t.knee = v,
    },
    Knob {
        id: "sound-lift-below",
        name: "Lift below",
        tip: "Sound quieter than this (whispers, quiet dialogue) is lifted.",
        range: range(-60.0, -20.0, 1.0),
        show: db,
        get: |t| t.lift_below,
        set: |t, v| t.lift_below = v,
    },
    Knob {
        id: "sound-lift-ratio",
        name: "Lift by",
        tip: "How much quiet sound is lifted. Off leaves it alone.",
        range: range(1.0, 5.0, 0.25),
        show: ratio,
        get: |t| t.lift_ratio,
        set: |t, v| t.lift_ratio = v,
    },
    Knob {
        id: "sound-boost",
        name: "Overall boost",
        tip: "Raises everything afterwards, to make up for what was turned down.",
        range: range(0.0, 15.0, 0.5),
        show: db,
        get: |t| t.boost,
        set: |t, v| t.boost = v,
    },
    Knob {
        id: "sound-ceiling",
        name: "Ceiling",
        tip: "Nothing gets louder than this, however sudden.",
        range: range(-12.0, 0.0, 0.5),
        show: db,
        get: |t| t.ceiling,
        set: |t, v| t.ceiling = v,
    },
];

const DOWNMIX_TIP: &str = "For 5.1 and 7.1 sound on stereo speakers or headphones: Dialogue first \
     puts the dialogue channel up front (Jellyfin's NightmodeDialogue); Balanced folds everything \
     in evenly (Dave750). Off leaves the mix to your speakers.";
const LEVELLER_TIP: &str = "Also evens out loudness over a few seconds (a community dynaudnorm \
     recipe). Takes a moment to settle after the filter switches on.";

fn profile_id(profile: SoundProfile) -> &'static str {
    match profile {
        SoundProfile::Preset(SoundPreset::Gentle) => "sound-gentle",
        SoundProfile::Preset(SoundPreset::Balanced) => "sound-balanced",
        SoundProfile::Preset(SoundPreset::Strong) => "sound-strong",
        SoundProfile::Preset(SoundPreset::Dialogue) => "sound-dialogue",
        SoundProfile::Preset(SoundPreset::Leveller) => "sound-leveller-preset",
        SoundProfile::Custom(0) => "sound-custom-1",
        SoundProfile::Custom(_) => "sound-custom-2",
    }
}

fn downmix_id(downmix: Downmix) -> &'static str {
    match downmix {
        Downmix::Off => "sound-downmix-off",
        Downmix::Dialogue => "sound-downmix-dialogue",
        Downmix::Balanced => "sound-downmix-balanced",
    }
}

/// `from` to `to` (RGBA), `p` of the way.
fn mix(from: u32, to: u32, p: f32) -> u32 {
    let channel = |shift: u32| {
        let (a, b) = ((from >> shift) & 0xff, (to >> shift) & 0xff);
        ((a as f32 + (b as f32 - a as f32) * p).round() as u32) << shift
    };
    channel(24) | channel(16) | channel(8) | channel(0)
}

const CHIP_OFF: u32 = 0xffffff0f;
const CHIP_ON: u32 = (ACCENT << 8) | 0x59;

impl Shell {
    /// The sound filters entry: closed, its name and the choice in use; open,
    /// everything to tune it.
    pub(super) fn sound_entry(&self, cx: &mut Context<Self>) -> AnyElement {
        let settings = self.sound_settings();
        let (open, moving) = {
            let revealed = self.revealed.borrow();
            (revealed.is_open(ENTRY), revealed.switch_moving(ENTRY))
        };
        let mut name = settings.profile.label();
        if matches!(settings.profile, SoundProfile::Preset(_)) && settings.changed() {
            name.push_str(", adjusted");
        }
        let summary = self.changing_text(
            "sound-summary",
            format!("{name} · also picked in the player, where Shift+N turns it on and off"),
            |text| div().text_sm().text_color(rgb(MUTED)).child(text),
        );
        let chevron = icon(Icon::ChevronDown)
            .size(px(16.0))
            .text_color(rgb(MUTED));
        let turned = move |p: f32| Transformation::rotate(radians(PI * p));
        let chevron = match moving.filter(|_| crate::motion::enabled()) {
            Some(n) => chevron
                .with_animation(
                    ("sound-chevron", n as usize),
                    Animation::new(FOLD.time()).with_easing(ease_in_out),
                    move |c, t| c.with_transformation(turned(if open { t } else { 1.0 - t })),
                )
                .into_any_element(),
            None => chevron
                .with_transformation(turned(if open { 1.0 } else { 0.0 }))
                .into_any_element(),
        };
        let header = div()
            .id(ENTRY)
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .px_4()
            .py_3()
            .cursor_pointer()
            .child(
                // The whole width, so a summary fading out (maybe longer
                // than the new one) stays on one line.
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .gap_1()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child("Sound filters"),
                    )
                    .child(summary),
            )
            .child(chevron)
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.revealed.borrow_mut().toggle_open(ENTRY);
                cx.notify();
            }));
        let body = (open || moving.is_some()).then(|| self.sound_fold(settings, open, moving, cx));
        div()
            .rounded_lg()
            .bg(rgba(0xffffff0a))
            .child(header)
            .children(body)
            .into_any_element()
    }

    /// What unfolds: the choices, what the picked one does, the knobs.
    /// Folding, it's clipped to a share of its measured height as it fades.
    fn sound_fold(
        &self,
        settings: SoundSettings,
        open: bool,
        moving: Option<u32>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let height = self.revealed.borrow_mut().fold_height(ENTRY);
        let measured = height.clone();
        let tuning = settings.tuning();
        let description = match settings.profile {
            SoundProfile::Preset(preset) => preset.description(),
            SoundProfile::Custom(_) => {
                "Your own tuning, kept while you try the presets. It starts as Night; hover a \
                 knob to see what it does."
            }
        };
        let chips = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .children(SoundProfile::ALL.map(|profile| {
                self.chip(
                    profile_id(profile),
                    profile.label(),
                    profile == settings.profile,
                    cx,
                    move |this, cx| this.pick_sound_profile(profile, cx),
                )
            }));
        let mut cells: Vec<AnyElement> = KNOBS
            .iter()
            .map(|knob| self.knob_cell(knob, (knob.get)(&tuning), cx))
            .collect();
        cells.push(self.downmix_cell(tuning.downmix, cx));
        cells.push(self.leveller_cell(tuning.leveller, cx));
        let body = div()
            .relative()
            .flex()
            .flex_col()
            .gap_3()
            .px_4()
            .pb_4()
            .child(chips)
            .child(
                self.changing_text("sound-description", description, |text| {
                    div().text_sm().text_color(rgb(MUTED)).child(text)
                }),
            )
            .child(
                div()
                    .grid()
                    .grid_cols(3)
                    .gap_x_6()
                    .gap_y_4()
                    .pt_1()
                    .children(cells),
            )
            .child(self.sound_footer(settings, cx))
            .child(
                canvas(
                    move |bounds, _, _| measured.set(f32::from(bounds.size.height)),
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            );
        let fold = div().overflow_hidden().child(body);
        match moving.filter(|_| crate::motion::enabled()) {
            Some(n) => fold
                .with_animation(
                    ("sound-fold", n as usize),
                    Animation::new(FOLD.time()).with_easing(ease_in_out),
                    move |fold, t| {
                        let shown = if open { t } else { 1.0 - t };
                        fold.max_h(px(height.get() * shown)).opacity(shown)
                    },
                )
                .into_any_element(),
            None => fold.into_any_element(),
        }
    }

    fn pick_sound_profile(&mut self, profile: SoundProfile, cx: &mut Context<Self>) {
        let old = self.sound_settings().profile;
        if old == profile {
            return;
        }
        self.change_sound(|settings| settings.profile = profile);
        let mut revealed = self.revealed.borrow_mut();
        revealed.switch_flipped(profile_id(old));
        revealed.switch_flipped(profile_id(profile));
        drop(revealed);
        cx.notify();
    }

    /// Changes the tuning in use (a preset's or a custom one).
    fn tune_sound(&mut self, change: impl FnOnce(&mut SoundTuning)) {
        self.change_sound(|settings| settings.tune(change));
    }

    /// Changes and saves the settings. What that changes on screen fades:
    /// the surround choice, the leveller switch, and the footer's hint or
    /// reset button as the tuning in use becomes changed or not. (Knobs
    /// glide by themselves.)
    fn change_sound(&mut self, change: impl FnOnce(&mut SoundSettings)) {
        let mut settings = self.sound_settings();
        let (was_changed, before) = (settings.changed(), settings.tuning());
        change(&mut settings);
        let after = settings.tuning();
        let mut revealed = self.revealed.borrow_mut();
        if settings.changed() != was_changed {
            revealed.switch_flipped(FOOTER);
        }
        if after.downmix != before.downmix {
            revealed.switch_flipped(downmix_id(before.downmix));
            revealed.switch_flipped(downmix_id(after.downmix));
        }
        if after.leveller != before.leveller {
            revealed.switch_flipped(LEVELLER);
        }
        drop(revealed);
        self.set_sound_settings(settings);
    }

    /// Under the knobs: while the tuning in use is as it comes, a hint;
    /// once changed, a way back. One fades into the other.
    fn sound_footer(&self, settings: SoundSettings, cx: &mut Context<Self>) -> AnyElement {
        let changed = settings.changed();
        let preset = matches!(settings.profile, SoundProfile::Preset(_));
        let hint = self.changing_text(
            "sound-footer-hint",
            if preset {
                "Turn any knob to adjust it; the original stays a click away."
            } else {
                "Turn any knob to make it your own."
            },
            |text| div().text_sm().text_color(rgb(MUTED)).child(text),
        );
        let reset = div()
            .id("sound-reset")
            .px_3()
            .py_1()
            .rounded_md()
            .text_sm()
            .cursor_pointer()
            .bg(rgba(0xffffff14))
            .hover(|s| s.bg(rgba(0xffffff26)))
            .child(self.changing_text(
                "sound-reset-label",
                if preset {
                    "Back to the original"
                } else {
                    "Start again from Night"
                },
                |text| div().child(text),
            ))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.change_sound(SoundSettings::reset);
                cx.notify();
            }));
        let moving = self
            .revealed
            .borrow()
            .switch_moving(FOOTER)
            .filter(|_| crate::motion::enabled());
        let layer = |child: AnyElement, shown: bool| {
            let layer = div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .child(child);
            match moving {
                Some(n) => layer
                    .with_animation(
                        (
                            if shown {
                                "sound-footer-in"
                            } else {
                                "sound-footer-out"
                            },
                            n as usize,
                        ),
                        Animation::new(SWITCH.time()).with_easing(ease_in_out),
                        move |l, t| l.opacity(if shown { t } else { 1.0 - t }),
                    )
                    .into_any_element(),
                None => layer.into_any_element(),
            }
        };
        let mut footer = div().relative().h(px(30.0));
        if changed || moving.is_some() {
            footer = footer.child(layer(reset.into_any_element(), changed));
        }
        if !changed || moving.is_some() {
            footer = footer.child(layer(hint, !changed));
        }
        footer.into_any_element()
    }

    /// A choice that's picked or not; the highlight fades as it changes.
    fn chip(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        selected: bool,
        cx: &mut Context<Self>,
        pick: impl Fn(&mut Shell, &mut Context<Shell>) + 'static,
    ) -> AnyElement {
        let chip: Stateful<Div> = div()
            .id(id)
            .px_3()
            .py_1()
            .rounded_md()
            .text_sm()
            .cursor_pointer()
            .child(label.into())
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| pick(this, cx)));
        let moving = self
            .revealed
            .borrow()
            .switch_moving(id)
            .filter(|_| crate::motion::enabled());
        match moving {
            Some(n) => chip
                .with_animation(
                    (id, n as usize),
                    Animation::new(SWITCH.time()).with_easing(ease_in_out),
                    move |c, t| {
                        c.bg(rgba(mix(
                            CHIP_OFF,
                            CHIP_ON,
                            if selected { t } else { 1.0 - t },
                        )))
                    },
                )
                .into_any_element(),
            None if selected => chip.bg(rgba(CHIP_ON)).into_any_element(),
            None => chip
                .bg(rgba(CHIP_OFF))
                .hover(|s| s.bg(rgba(0xffffff1f)))
                .into_any_element(),
        }
    }

    /// A knob: its name and value over a slider, the rest in its tooltip.
    fn knob_cell(&self, knob: &Knob, value: f32, cx: &mut Context<Self>) -> AnyElement {
        let (id, range, set) = (knob.id, knob.range, knob.set);
        let fraction = range.fraction(value);
        let glide = self
            .revealed
            .borrow_mut()
            .glide(id, value, self.slider_drag == Some(id))
            .filter(|_| crate::motion::enabled())
            .map(|(from, n)| (range.fraction(from), n));
        let tracks = self.slider_tracks.clone();
        let parts = move |f: f32| {
            [
                div()
                    .absolute()
                    .left_0()
                    .top(px(KNOB / 2.0 - 2.0))
                    .w(relative(f))
                    .h(px(4.0))
                    .rounded_full()
                    .bg(rgb(ACCENT))
                    .into_any_element(),
                div()
                    .absolute()
                    .top_0()
                    .left(relative(f))
                    .ml(px(-KNOB / 2.0))
                    .size(px(KNOB))
                    .rounded_full()
                    .bg(rgb(0xffffff))
                    .into_any_element(),
            ]
        };
        let track = div()
            .relative()
            .mx(px(KNOB / 2.0))
            .h(px(KNOB))
            .child(
                canvas(
                    move |bounds, _, _| {
                        tracks.borrow_mut().insert(id, bounds);
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
            );
        let track = match glide {
            Some((from, n)) => track
                .with_animation(
                    (id, n as usize),
                    Animation::new(SWITCH.time()).with_easing(ease_in_out),
                    move |track, t| track.children(parts(from + (fraction - from) * t)),
                )
                .into_any_element(),
            None => track.children(parts(fraction)).into_any_element(),
        };
        let pick = move |this: &mut Shell, x: gpui::Pixels| {
            let Some(bounds) = this.slider_tracks.borrow().get(id).copied() else {
                return;
            };
            let value = range.at((x - bounds.left()) / bounds.size.width);
            this.tune_sound(|t| set(t, value));
        };
        let (cell, tip) = self.tip_cell(SharedString::from(format!("{id}-cell")), id, knob.tip, cx);
        let cell = cell
            .flex()
            .flex_col()
            .gap_1p5()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .gap_2()
                    .text_sm()
                    .child(knob.name)
                    .child(div().text_color(rgb(MUTED)).child((knob.show)(value))),
            )
            .child(track)
            .cursor_pointer()
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
                    pick(this, event.position.x);
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
            );
        with_tip(cell, tip).into_any_element()
    }

    fn downmix_cell(&self, downmix: Downmix, cx: &mut Context<Self>) -> AnyElement {
        let choices = div().flex().gap_1().children(Downmix::ALL.map(|choice| {
            self.chip(
                downmix_id(choice),
                choice.label(),
                choice == downmix,
                cx,
                move |this, cx| {
                    this.tune_sound(|t| t.downmix = choice);
                    cx.notify();
                },
            )
        }));
        let (cell, tip) = self.tip_cell(
            "sound-downmix-cell".into(),
            "sound-downmix",
            DOWNMIX_TIP,
            cx,
        );
        let cell = cell
            .flex()
            .flex_col()
            .gap_1p5()
            .child(div().text_sm().child("Surround to stereo"))
            .child(choices);
        with_tip(cell, tip).col_span(2).into_any_element()
    }

    fn leveller_cell(&self, on: bool, cx: &mut Context<Self>) -> AnyElement {
        const ID: &str = LEVELLER;
        let (cell, tip) = self.tip_cell("sound-leveller-cell".into(), ID, LEVELLER_TIP, cx);
        let cell = cell
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .text_sm()
            .child("Leveller")
            .child(self.switch(ID, on))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.tune_sound(|t| t.leveller = !on);
                cx.notify();
            }));
        with_tip(cell, tip).into_any_element()
    }

    /// A grid cell that shows `text` in a tooltip while the mouse is on it,
    /// and the tooltip (placed under the cell by [`with_tip`]).
    fn tip_cell(
        &self,
        cell_id: SharedString,
        tip_id: &'static str,
        text: &'static str,
        cx: &mut Context<Self>,
    ) -> (Stateful<Div>, Option<AnyElement>) {
        let tip = match self.revealed.borrow().tip(tip_id) {
            Tip::Hidden => None,
            Tip::Showing(n) => Some(
                tip_card(text)
                    .with_animation(
                        ("tip-in", n as usize),
                        Animation::new(TIP_DELAY + APPEAR.time()),
                        |card, t| {
                            let total = TIP_DELAY + APPEAR.time();
                            card.opacity(tip_shown(total.mul_f32(t)))
                        },
                    )
                    .into_any_element(),
            ),
            Tip::Leaving(from, n) => Some(
                tip_card(text)
                    .with_animation(
                        ("tip-out", n as usize),
                        Animation::new(APPEAR.time()).with_easing(ease_in_out),
                        move |card, t| card.opacity(from * (1.0 - t)),
                    )
                    .into_any_element(),
            ),
        };
        let cell = div()
            .id(cell_id)
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.revealed.borrow_mut().hover_tip(tip_id, *hovered);
                cx.notify();
            }));
        let tip = tip.map(|tip| {
            div()
                .absolute()
                .top_full()
                .left_0()
                .child(
                    deferred(
                        anchored()
                            .snap_to_window_with_margin(px(8.0))
                            .child(div().pt_1().child(tip)),
                    )
                    .with_priority(2),
                )
                .into_any_element()
        });
        (cell, tip)
    }
}

/// A grid cell with its tooltip under it. The tooltip sits beside the
/// cell, not in it, so it never takes on an opacity the cell is given (a
/// deferred element keeps the opacity of where it was made).
fn with_tip(cell: Stateful<Div>, tip: Option<AnyElement>) -> Div {
    div()
        .relative()
        .flex()
        .flex_col()
        .child(cell.flex_1())
        .children(tip)
}

fn tip_card(text: &'static str) -> Div {
    div()
        .w(px(280.0))
        .px_3()
        .py_2()
        .rounded_md()
        .bg(rgb(0x1c1f27))
        .border_1()
        .border_color(rgba(0xffffff1f))
        .shadow_lg()
        .text_xs()
        .text_color(rgb(0xf4f5f7))
        .child(text)
}
