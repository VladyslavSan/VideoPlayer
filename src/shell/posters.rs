//! How bright posters are: a little dimmed while browsing, lit up under
//! the mouse (or the arrow keys' cursor), easing between the two. Both
//! levels are settings (Settings → Appearance, `settings/appearance_page.rs`).

use std::time::Instant;

use gpui::{AnyElement, SharedString, div, prelude::*, rgba};

/// The sliders' range.
pub(super) const DIMMEST: f32 = 0.4;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Light {
    pub rest: f32,
    pub lit: f32,
}

impl Light {
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

/// The darkening laid over a card's picture to make it `brightness`.
pub(super) fn shade(brightness: f32) -> AnyElement {
    let alpha = ((1.0 - brightness).clamp(0.0, 1.0) * 255.0).round() as u32;
    div()
        .absolute()
        .inset_0()
        .bg(rgba(alpha))
        .into_any_element()
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
