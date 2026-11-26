//! The tracks panel: audio tracks, subtitles, and the options (speed,
//! subtitle size and delay, sound filter, streaming quality).

use gpui::{
    AnyElement, ClickEvent, Context, Div, ElementId, FontWeight, MouseDownEvent, SharedString,
    Stateful, Window, div, prelude::*, px, rgb, rgba,
};

use super::{ACCENT, MUTED, format_delay, format_speed};
use crate::jellyfin::{QUALITIES, Quality};
use crate::player::{Player, tracks};
use crate::ui::icons::{Icon, icon};
use crate::ui::menu;
use crate::video::{SoundProfile, Track, TrackKind};

/// The tallest a column of tracks gets before it scrolls.
const COLUMN_MAX_HEIGHT: f32 = 280.0;
/// What lies under the mouse on a row.
const ROW_HOVER: u32 = 0xffffff14;

/// A row to pick in the tracks panel: a track, or "Off".
struct ChoiceRow {
    id: ElementId,
    primary: SharedString,
    secondary: SharedString,
    selected: bool,
}

impl ChoiceRow {
    /// The row for the `number`th track of its kind.
    fn of_track(name: &'static str, track: &Track, number: usize) -> Self {
        let label = tracks::label(track, number);
        Self {
            id: ElementId::NamedInteger(name.into(), track.id as u64),
            primary: label.primary.into(),
            secondary: label.secondary.into(),
            selected: track.selected,
        }
    }
}

impl Player {
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
                    .max_h(px(COLUMN_MAX_HEIGHT))
                    .overflow_y_scroll()
                    .children(rows),
            )
            .into_any_element()
    }

    fn choice_row(
        &self,
        row: ChoiceRow,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        let ChoiceRow {
            id,
            primary,
            secondary,
            selected,
        } = row;
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
            .hover(|s| s.bg(rgba(ROW_HOVER)))
            .child(
                div()
                    .size(px(18.0))
                    .flex_none()
                    .pt_0p5()
                    .child(self.fades.swaps.draw(
                        check,
                        if selected { "on" } else { "off" },
                        |state| {
                            div().size(px(16.0)).when(state == "on", |d| {
                                d.child(icon(Icon::Check).size(px(16.0)).text_color(rgb(ACCENT)))
                            })
                        },
                    )),
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
        let small = |button: Stateful<Div>| button.size(px(30.0));
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
                        cx,
                        move |this, _, cx| down(this, -1, cx),
                    )))
                    .child(
                        div().flex_1().flex().justify_center().text_sm().child(
                            self.fades
                                .swaps
                                .draw(format!("{id}-value"), value, |value| div().child(value)),
                        ),
                    )
                    .child(small(self.icon_button(
                        (id, 1usize),
                        Icon::Plus,
                        16.0,
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
        let lit = self.tracks.quality.menu.lit(|_| true);
        let mut picker = div().relative().flex().child(
            menu::trigger(
                "quality",
                self.fades
                    .swaps
                    .draw("quality", current.label(), |label| div().child(label)),
                lit,
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.tracks.quality.toggle_menu(current);
                this.poke(cx);
            })),
        );
        for shown in self.tracks.quality.menu.drawn() {
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
                    this.tracks.quality.menu.close();
                    this.poke(cx);
                }),
            ));
        }
        Self::menu_option("Streaming quality", picker)
    }

    /// "Sound filter" with a button that opens a menu: off, the presets
    /// and the custom tunings.
    fn sound_picker(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let current = self.tracks.sound.choice();
        let label = current.map_or_else(|| "Off".to_string(), |profile| profile.label());
        let lit = self.tracks.sound.menu.lit(|_| true);
        let mut picker = div().relative().flex().child(
            menu::trigger(
                "sound",
                self.fades
                    .swaps
                    .draw("sound", label, |label| div().child(label)),
                lit,
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.tracks.sound.toggle_menu(current);
                this.poke(cx);
            })),
        );
        for shown in self.tracks.sound.menu.drawn() {
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
                    this.tracks.sound.menu.close();
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

    pub(super) fn track_panel(&self, window: &Window, cx: &mut Context<Self>) -> Stateful<Div> {
        Self::panel_frame("tracks-panel")
            .flex()
            .gap_3()
            .child(Self::panel_column("Audio", self.audio_rows(cx)))
            .child(Self::panel_column("Subtitles", self.subtitle_rows(cx)))
            .child(Self::panel_column("Options", self.option_rows(window, cx)))
    }

    fn audio_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = Vec::new();
        for (i, track) in self.tracks.of(TrackKind::Audio).enumerate() {
            let id = track.id;
            let row = ChoiceRow::of_track("audio", track, i + 1);
            rows.push(self.choice_row(row, cx, move |this, cx| this.select_audio(id, cx)));
        }
        if rows.is_empty() {
            rows.push(
                div()
                    .px_2()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child("None")
                    .into_any_element(),
            );
        }
        rows
    }

    /// "Off", then the subtitle tracks.
    fn subtitle_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let any_subtitle = self.tracks.of(TrackKind::Subtitle).any(|t| t.selected);
        let off = ChoiceRow {
            id: "subtitle-off".into(),
            primary: "Off".into(),
            secondary: SharedString::default(),
            selected: !any_subtitle,
        };
        let mut rows = vec![self.choice_row(off, cx, |this, cx| this.select_subtitle(None, cx))];
        for (i, track) in self.tracks.of(TrackKind::Subtitle).enumerate() {
            let id = track.id;
            let row = ChoiceRow::of_track("subtitle", track, i + 1);
            rows.push(self.choice_row(row, cx, move |this, cx| this.select_subtitle(Some(id), cx)));
        }
        rows
    }

    /// Speed, subtitle size and delay, the sound filter and, for a Jellyfin
    /// video, the streaming quality.
    fn option_rows(&self, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let status = self.status;
        let mut options = vec![
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
        options.push(self.sound_picker(window, cx));
        if let Some(quality) = self.tracks.quality.current {
            options.push(self.quality_picker(quality, window, cx));
        }
        options
    }
}
