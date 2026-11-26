//! Fading the player's overlays in and out, so nothing on it pops.
//!
//! Overlays often turn back halfway (the bar starts fading out, the mouse moves): a
//! `Fade` (`motion`) eases from wherever it is, so a reversal never jumps. Its value comes
//! from the clock when the player draws, which it does every frame while one is `moving`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Instant;

use gpui::{
    Animation, AnimationExt, AnyElement, IntoElement, ParentElement, SharedString, Styled, div,
};

use super::panels::Panel;
use crate::motion::{Fade, OVERLAY, ease_in_out};

/// What the up-next card last showed: its countdown, and whether the video
/// had ended (rather than being in its credits).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpNextShown {
    pub seconds: u64,
    pub ended: bool,
}

/// What things that come and go last showed: drawn so while they fade out.
pub struct LastShown {
    pub up_next: UpNextShown,
    /// The skip button's label.
    pub skip: &'static str,
    /// The last speed other than normal, for the speed chip.
    pub speed: f64,
}

/// The player's overlays, each fading in and out.
pub struct Fades {
    /// The control bar and the title.
    pub controls: Fade,
    /// Each panel, by `Panel::index`.
    panels: [Fade; 3],
    pub up_next: Fade,
    pub skip: Fade,
    /// The seek bar's time preview (and the bar thickening under it).
    pub preview: Fade,
    /// The speed chip in the control bar, when not at normal speed.
    pub speed: Fade,
    /// The video, as the black backdrop lifts off it.
    pub video: Fade,
    pub shown: LastShown,
    /// Icons and values that dissolve when they change.
    pub swaps: Swaps,
}

impl Fades {
    pub fn new() -> Self {
        Self {
            controls: Fade::new(true),
            panels: [Fade::new(false); 3],
            up_next: Fade::new(false),
            skip: Fade::new(false),
            preview: Fade::new(false),
            speed: Fade::new(false),
            video: Fade::new(false),
            shown: LastShown {
                up_next: UpNextShown {
                    seconds: 0,
                    ended: false,
                },
                skip: "",
                speed: 1.0,
            },
            swaps: Swaps::default(),
        }
    }

    pub fn panel(&self, panel: Panel) -> &Fade {
        &self.panels[panel.index()]
    }

    /// Points each panel's fade at whether it is the one open.
    pub fn set_open_panel(&mut self, open: Option<Panel>) {
        for panel in Panel::ALL {
            self.panels[panel.index()].set(open == Some(panel));
        }
    }

    pub fn moving(&self) -> bool {
        [
            &self.controls,
            &self.up_next,
            &self.skip,
            &self.preview,
            &self.speed,
            &self.video,
        ]
        .into_iter()
        .chain(&self.panels)
        .any(Fade::moving)
    }
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
