//! Fading the player's overlays in and out, so nothing on it pops.
//!
//! The overlays come and go with the mouse, and often turn back halfway
//! (the bar starts fading out, the mouse moves): a `Fade` eases from
//! wherever it is to where it's going, so a reversal never jumps. Its value
//! is computed from the clock when the player draws; the player keeps
//! drawing every frame while one is on its way (`moving`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt, AnyElement, IntoElement, ParentElement, SharedString, Styled, div,
};

use crate::motion::{OVERLAY, ease_in_out};

/// Something that's shown or not, fading between the two.
#[derive(Debug, Clone, Copy)]
pub struct Fade {
    shown: bool,
    /// Where it started from when it last turned.
    from: f32,
    at: Instant,
}

impl Fade {
    pub fn new(shown: bool) -> Self {
        Self {
            shown,
            from: if shown { 1.0 } else { 0.0 },
            at: Instant::now(),
        }
    }

    /// Starts fading in or out, from wherever it is now.
    pub fn set(&mut self, shown: bool) {
        if shown != self.shown {
            self.from = self.value();
            self.shown = shown;
            self.at = Instant::now();
        }
    }

    pub fn shown(&self) -> bool {
        self.shown
    }

    /// How visible it is now: 0 (gone) to 1.
    pub fn value(&self) -> f32 {
        if !crate::motion::enabled() {
            return self.target();
        }
        fade_value(self.from, self.target(), self.at.elapsed(), OVERLAY.time())
    }

    /// Still to be drawn: shown, or fading out.
    pub fn on_screen(&self) -> bool {
        self.shown || self.value() > 0.0
    }

    /// On its way in or out.
    pub fn moving(&self) -> bool {
        self.value() != self.target()
    }

    fn target(&self) -> f32 {
        if self.shown { 1.0 } else { 0.0 }
    }
}

/// From `from` toward `to`, `elapsed` into a fade that takes `full` to
/// cross the whole way (a shorter way takes less).
fn fade_value(from: f32, to: f32, elapsed: Duration, full: Duration) -> f32 {
    let span = (to - from).abs();
    let total = full.as_secs_f32() * span;
    if total <= 0.0 {
        return to;
    }
    let progress = elapsed.as_secs_f32() / total;
    if progress >= 1.0 {
        // Exactly there, so it stops counting as moving.
        return to;
    }
    from + (to - from) * ease_in_out(progress.max(0.0))
}

/// Short notes in one place ("Volume 80%", "Loading…"): a new one fades
/// in over the one before as that fades out.
#[derive(Default)]
pub struct Notes {
    notes: Vec<(SharedString, Fade)>,
}

/// Notes still fading out are dropped beyond this many (quick key repeats).
const MAX_NOTES: usize = 4;

impl Notes {
    /// Shows `text`, or nothing; the same text again just stays.
    pub fn show(&mut self, text: Option<SharedString>) {
        if self.current() == text.as_ref() {
            return;
        }
        for (_, fade) in &mut self.notes {
            fade.set(false);
        }
        if let Some(text) = text {
            let mut fade = Fade::new(false);
            fade.set(true);
            self.notes.push((text, fade));
        }
        if self.notes.len() > MAX_NOTES {
            self.notes.drain(..self.notes.len() - MAX_NOTES);
        }
    }

    /// The note being shown (not one fading out).
    pub fn current(&self) -> Option<&SharedString> {
        self.notes
            .last()
            .filter(|(_, fade)| fade.shown())
            .map(|(text, _)| text)
    }

    /// What to draw, oldest first, with how visible each is.
    pub fn drawn(&mut self) -> Vec<(SharedString, f32)> {
        self.notes.retain(|(_, fade)| fade.on_screen());
        self.notes
            .iter()
            .map(|(text, fade)| (text.clone(), fade.value()))
            .collect()
    }

    pub fn moving(&self) -> bool {
        self.notes.iter().any(|(_, fade)| fade.moving())
    }
}

/// What a swapping thing shows, what it showed before, and when that
/// changed.
struct Swap {
    key: SharedString,
    before: Option<SharedString>,
    at: Instant,
}

/// Things that change in place (an icon, a value): when one does, the old
/// look dissolves into the new. Each is remembered by an id.
#[derive(Default)]
pub struct Swaps {
    shown: RefCell<HashMap<SharedString, Swap>>,
}

impl Swaps {
    /// `draw(key)`, dissolving from the previous key's look for a moment
    /// after `key` changes. Both looks are drawn centred on each other.
    pub fn draw<E>(
        &self,
        id: impl Into<SharedString>,
        key: impl Into<SharedString>,
        draw: impl Fn(SharedString) -> E,
    ) -> AnyElement
    where
        E: IntoElement + Styled + 'static,
    {
        let (id, key) = (id.into(), key.into());
        let (before, at) = {
            let mut shown = self.shown.borrow_mut();
            let swap = shown.entry(id.clone()).or_insert_with(|| Swap {
                key: key.clone(),
                before: None,
                at: Instant::now(),
            });
            if swap.key != key {
                *swap = Swap {
                    key: key.clone(),
                    before: Some(swap.key.clone()),
                    at: Instant::now(),
                };
            }
            (swap.before.clone(), swap.at)
        };
        let Some(before) =
            before.filter(|_| crate::motion::enabled() && at.elapsed() < OVERLAY.time())
        else {
            return draw(key).into_any_element();
        };
        let fade = || Animation::new(OVERLAY.time()).with_easing(ease_in_out);
        div()
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .child(draw(key.clone()).with_animation(
                SharedString::from(format!("swap-in-{id}-{key}")),
                fade(),
                |look, t| look.opacity(t),
            ))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(draw(before.clone()).with_animation(
                        SharedString::from(format!("swap-out-{id}-{before}")),
                        fade(),
                        |look, t| look.opacity(1.0 - t),
                    )),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: Duration = Duration::from_millis(400);

    #[test]
    fn fades_ease_all_the_way() {
        assert_eq!(fade_value(0.0, 1.0, Duration::ZERO, FULL), 0.0);
        assert_eq!(fade_value(0.0, 1.0, FULL / 2, FULL), 0.5);
        assert_eq!(fade_value(0.0, 1.0, FULL, FULL), 1.0);
        assert_eq!(fade_value(1.0, 0.0, FULL * 2, FULL), 0.0);
    }

    #[test]
    fn turning_back_halfway_takes_half_as_long() {
        // From halfway out, back in: done in half the time, no jump.
        assert_eq!(fade_value(0.5, 1.0, Duration::ZERO, FULL), 0.5);
        assert_eq!(fade_value(0.5, 1.0, FULL / 2, FULL), 1.0);
        assert_eq!(fade_value(0.3, 1.0, FULL, FULL), 1.0);
        assert!(fade_value(0.5, 1.0, FULL / 4, FULL) > 0.5);
    }

    #[test]
    fn notes_replace_each_other() {
        let mut notes = Notes::default();
        notes.show(Some("Volume 80%".into()));
        notes.show(Some("Volume 80%".into()));
        assert_eq!(notes.notes.len(), 1);
        notes.show(Some("Volume 85%".into()));
        assert_eq!(notes.current().map(|s| s.as_ref()), Some("Volume 85%"));
        assert!(!notes.notes[0].1.shown());
        notes.show(None);
        assert_eq!(notes.current(), None);
        for i in 0..10 {
            notes.show(Some(format!("Volume {i}").into()));
        }
        assert!(notes.notes.len() <= MAX_NOTES);
    }
}
