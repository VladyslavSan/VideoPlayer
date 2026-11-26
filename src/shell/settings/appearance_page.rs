//! Settings → Appearance: how bright posters are while browsing and under
//! the mouse, with a sample of each. The shell's cards are told of a change.

use gpui::{AnyElement, Context, div, prelude::*, px, rgb};

use super::{Event, Settings};
use crate::shell::posters::{self, DIMMEST, Light};
use crate::shell::sliders::{Range, SliderRow};
use crate::shell::{MUTED, widgets};

/// Poster brightness, as a fraction.
const BRIGHTNESS: Range = Range {
    min: DIMMEST,
    max: 1.0,
    step: 0.01,
};

fn percent(value: f32) -> String {
    format!("{:.0}%", value * 100.0)
}

/// A picture-sized patch of colour at `brightness`, over its label.
fn sample(label: &'static str, brightness: f32) -> gpui::Div {
    let patch = div()
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
        .child(posters::shade(brightness));
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(patch)
        .child(div().text_sm().text_color(rgb(MUTED)).child(label))
}

impl Settings {
    /// Saves how bright posters are and tells the shell, which draws them.
    fn set_poster_light(&mut self, light: Light, cx: &mut Context<Self>) {
        let light = Light {
            rest: light.rest.clamp(DIMMEST, 1.0),
            lit: light.lit.clamp(DIMMEST, 1.0),
        };
        let previous = self.poster_light;
        self.poster_light = light;
        self.preferences().set_poster_light(light, previous);
        cx.emit(Event::PosterLightChanged { light });
    }

    fn set_rest_brightness(&mut self, rest: f32, cx: &mut Context<Self>) {
        let light = Light {
            rest,
            ..self.poster_light
        };
        self.set_poster_light(light, cx);
    }

    fn set_lit_brightness(&mut self, lit: f32, cx: &mut Context<Self>) {
        let light = Light {
            lit,
            ..self.poster_light
        };
        self.set_poster_light(light, cx);
    }

    pub(super) fn appearance_page(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let light = self.poster_light;
        let rest = SliderRow {
            id: "setting-poster-rest",
            title: "Brightness",
            description: "How bright posters and pictures are while you browse.",
            value: light.rest,
            range: BRIGHTNESS,
            show: percent,
        };
        let lit = SliderRow {
            id: "setting-poster-lit",
            title: "Under the mouse",
            description: "How bright the one under the mouse, or picked with the arrow keys, gets.",
            value: light.lit,
            range: BRIGHTNESS,
            show: percent,
        };
        // What the two look like, side by side.
        let samples = div()
            .flex()
            .gap_5()
            .pt_3()
            .px_4()
            .child(sample("While browsing", light.rest))
            .child(sample("Under the mouse", light.lit));
        let body = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.sliders.row(rest, cx, Self::set_rest_brightness))
            .child(self.sliders.row(lit, cx, Self::set_lit_brightness))
            .child(samples);
        vec![widgets::section("Posters", body)]
    }
}
