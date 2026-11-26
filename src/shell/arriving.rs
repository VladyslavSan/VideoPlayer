//! Things that arrive after their page opened (Sonarr's answer, an
//! overview, a list) fade in rather than pop; things already there when the
//! page opened just come with the page's own fade. A "loading" note fades
//! in only once the wait is long enough to notice, so a quick answer
//! never flashes one; if it did show, it fades out as what it waited for
//! fades in.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt, AnyElement, IntoElement, ParentElement, SharedString, Styled, div,
};

use super::Shell;
use crate::motion::{APPEAR, WAIT};

/// A tooltip `elapsed` after the mouse went onto its thing: nothing for a
/// moment, then fading in.
pub(super) fn tip_shown(elapsed: Duration) -> f32 {
    let t = (elapsed.saturating_sub(TIP_DELAY)).as_secs_f32() / APPEAR.time().as_secs_f32();
    crate::motion::ease_in_out(t.min(1.0))
}

/// Shown this soon after its page opened, it came with the page.
const WITH_THE_PAGE: Duration = Duration::from_millis(30);

/// What each page has shown so far: whether each thing fades in.
#[derive(Default)]
pub(super) struct Revealed {
    since: Option<Instant>,
    seen: HashMap<SharedString, bool>,
    /// Loading notes, from when they were first drawn.
    waits: HashMap<SharedString, Instant>,
    /// How visible a note was when what it waited for arrived.
    replaced: HashMap<SharedString, f32>,
    /// Text that changes in place: what it says, what it said, since when.
    texts: HashMap<SharedString, (SharedString, Option<SharedString>, Instant)>,
    /// Switches, by id: how many times each was flipped, and when last
    /// (kept across pages, so a switch only moves when it's flipped).
    switches: HashMap<SharedString, (u32, Instant)>,
    /// Entries that unfold (sound filters in Settings), by id: open or not.
    open: HashMap<&'static str, bool>,
    /// Their content's natural height, measured as they draw.
    fold_heights: HashMap<&'static str, Rc<Cell<f32>>>,
    /// Sliders, by id: (value, value it glides from, since when, how many
    /// glides), so a value set from elsewhere (a preset) glides there.
    glides: HashMap<&'static str, (f32, f32, Instant, u32)>,
    /// The tooltip asked for (by hovering), since when, and which time
    /// (to key its animation).
    tip: Option<(&'static str, Instant, u32)>,
    /// The one before, fading out: how visible it was, and which time.
    tip_leaving: Option<(&'static str, f32, u32)>,
    tips: u32,
}

/// Where a tooltip is.
#[derive(Clone, Copy)]
pub(super) enum Tip {
    Hidden,
    /// Waiting, then fading in; the number keys its animation.
    Showing(u32),
    /// Fading out from this visible, keyed by the number.
    Leaving(f32, u32),
}

/// A tooltip waits this long under the mouse before fading in.
pub(super) const TIP_DELAY: Duration = Duration::from_millis(450);

impl Revealed {
    pub(super) fn is_open(&self, id: &'static str) -> bool {
        self.open.get(id).copied().unwrap_or(false)
    }

    /// Opens or closes `id`; its fold moves like a switch.
    pub(super) fn toggle_open(&mut self, id: &'static str) {
        let open = !self.is_open(id);
        self.open.insert(id, open);
        self.switch_flipped(id);
    }

    pub(super) fn fold_height(&mut self, id: &'static str) -> Rc<Cell<f32>> {
        self.fold_heights.entry(id).or_default().clone()
    }

    /// Where a slider's knob is drawn from: `value`, or, just after it
    /// changed other than by dragging, gliding there from the old one
    /// (`Some((from, glides))`, the count keying its animation).
    pub(super) fn glide(
        &mut self,
        id: &'static str,
        value: f32,
        dragging: bool,
    ) -> Option<(f32, u32)> {
        let now = Instant::now();
        let entry = self.glides.entry(id).or_insert((value, value, now, 0));
        if entry.0 != value {
            *entry = if dragging {
                (value, value, entry.2, entry.3)
            } else {
                (value, entry.0, now, entry.3 + 1)
            };
        }
        (entry.1 != entry.0 && entry.2.elapsed() < crate::motion::SWITCH.time())
            .then_some((entry.1, entry.3))
    }

    /// The mouse went onto (`true`) or off something with a tooltip.
    pub(super) fn hover_tip(&mut self, id: &'static str, hovered: bool) {
        let current = self.tip.is_some_and(|(tip, _, _)| tip == id);
        if hovered && !current {
            self.hover_tip_off();
            self.tips += 1;
            self.tip = Some((id, Instant::now(), self.tips));
        } else if !hovered && current {
            self.hover_tip_off();
        }
    }

    fn hover_tip_off(&mut self) {
        if let Some((id, since, _)) = self.tip.take() {
            let shown = tip_shown(since.elapsed());
            if shown > 0.0 {
                self.tips += 1;
                self.tip_leaving = Some((id, shown, self.tips));
            }
        }
    }

    pub(super) fn tip(&self, id: &'static str) -> Tip {
        match (self.tip, self.tip_leaving) {
            (Some((tip, _, n)), _) if tip == id => Tip::Showing(n),
            (_, Some((tip, from, n))) if tip == id => Tip::Leaving(from, n),
            _ => Tip::Hidden,
        }
    }

    pub(super) fn switch_flipped(&mut self, id: &str) {
        let entry = self
            .switches
            .entry(SharedString::from(id.to_string()))
            .or_insert((0, Instant::now()));
        *entry = (entry.0 + 1, Instant::now());
    }

    /// For a switch still on its way across: how many times it was
    /// flipped (to key its animation). None once it's at rest.
    pub(super) fn switch_moving(&self, id: &str) -> Option<u32> {
        self.switches
            .get(id)
            .filter(|(_, at)| at.elapsed() < crate::motion::SWITCH.time())
            .map(|(flips, _)| *flips)
    }

    /// A new page: everything on it is new.
    pub(super) fn page_opened(&mut self) {
        self.since = Some(Instant::now());
        self.seen.clear();
        self.waits.clear();
        self.replaced.clear();
        self.texts.clear();
    }

    /// How visible the loading note `id` is now (0 if it never showed).
    fn note_shown(&self, id: &SharedString) -> f32 {
        self.waits.get(id).map_or(0.0, |started| {
            note_opacity(started.elapsed().as_secs_f32(), delay(), fade())
        })
    }

    /// Whether `id` arrived after its page did, deciding the first time
    /// it's asked.
    fn late(&mut self, id: &SharedString) -> bool {
        let since = self.since;
        *self
            .seen
            .entry(id.clone())
            .or_insert_with(|| since.is_some_and(|since| since.elapsed() > WITH_THE_PAGE))
    }
}

fn delay() -> f32 {
    WAIT.time().as_secs_f32()
}

fn fade() -> f32 {
    APPEAR.time().as_secs_f32()
}

/// A loading note's opacity `elapsed` seconds after it was first drawn.
fn note_opacity(elapsed: f32, delay: f32, fade: f32) -> f32 {
    crate::motion::ease_in_out(((elapsed - delay) / fade).clamp(0.0, 1.0))
}

impl Shell {
    /// Text that can change while it's on screen (a status, a label): when
    /// it does, the old text fades out as the new fades in. `draw` styles
    /// either.
    pub(super) fn changing_text<E>(
        &self,
        id: impl Into<SharedString>,
        text: impl Into<SharedString>,
        draw: impl Fn(SharedString) -> E,
    ) -> AnyElement
    where
        E: IntoElement + Styled + 'static,
    {
        let id = id.into();
        let text = text.into();
        if !crate::motion::enabled() || self.drawing_leaving.get() {
            return draw(text).into_any_element();
        }
        let (before, at) = {
            let mut revealed = self.revealed.borrow_mut();
            let entry = revealed
                .texts
                .entry(id.clone())
                .or_insert_with(|| (text.clone(), None, Instant::now()));
            if entry.0 != text {
                *entry = (text.clone(), Some(entry.0.clone()), Instant::now());
            }
            (entry.1.clone(), entry.2)
        };
        let Some(before) = before.filter(|_| at.elapsed() < APPEAR.time()) else {
            return draw(text).into_any_element();
        };
        let key = |text: &SharedString| {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            text.hash(&mut hasher);
            hasher.finish()
        };
        let fade = || Animation::new(APPEAR.time()).with_easing(crate::motion::ease_in_out);
        div()
            .relative()
            .min_w(gpui::px(0.0))
            .child(draw(text.clone()).with_animation(
                SharedString::from(format!("text-in-{id}-{}", key(&text))),
                fade(),
                |text, t| text.opacity(t),
            ))
            .child(
                draw(before.clone())
                    .absolute()
                    .top_0()
                    .left_0()
                    .with_animation(
                        SharedString::from(format!("text-out-{id}-{}", key(&before))),
                        fade(),
                        |text, t| text.opacity(1.0 - t),
                    ),
            )
            .into_any_element()
    }

    /// `content`, fading in if it arrived after the page opened. `id` names
    /// it on the page (say which state it shows, if it changes).
    pub(super) fn arriving<E>(&self, id: impl Into<SharedString>, content: E) -> AnyElement
    where
        E: IntoElement + Styled + 'static,
    {
        let id = id.into();
        if !crate::motion::enabled()
            || self.drawing_leaving.get()
            || !self.revealed.borrow_mut().late(&id)
        {
            return content.into_any_element();
        }
        content
            .with_animation(
                SharedString::from(format!("reveal-{id}")),
                Animation::new(APPEAR.time()).with_easing(crate::motion::ease_in_out),
                |content, t| content.opacity(t),
            )
            .into_any_element()
    }

    /// A note that something is loading: it fades in only once the wait
    /// has gone on a moment.
    pub(super) fn waiting<E>(&self, id: impl Into<SharedString>, note: E) -> AnyElement
    where
        E: IntoElement + Styled + 'static,
    {
        if !crate::motion::enabled() || self.drawing_leaving.get() {
            return note.into_any_element();
        }
        let (delay, fade) = (delay(), fade());
        let total = delay + fade;
        let id = id.into();
        self.revealed
            .borrow_mut()
            .waits
            .entry(id.clone())
            .or_insert_with(Instant::now);
        note.with_animation(
            SharedString::from(format!("waiting-{id}-{}", self.page_generation)),
            Animation::new(Duration::from_secs_f32(total)),
            move |note, t| note.opacity(note_opacity(t * total, delay, fade)),
        )
        .into_any_element()
    }

    /// `content`, arriving where the loading note `note_id` was (`note` is
    /// that note, drawn the same way): if the note had shown, it fades out
    /// over `content` fading in, rather than vanishing.
    pub(super) fn arriving_over<N, E>(
        &self,
        note_id: impl Into<SharedString>,
        note: N,
        id: impl Into<SharedString>,
        content: E,
    ) -> AnyElement
    where
        N: IntoElement + Styled + 'static,
        E: IntoElement + Styled + 'static,
    {
        let id = id.into();
        let note_id = note_id.into();
        let content = self.arriving(id.clone(), content);
        let shown = {
            let mut revealed = self.revealed.borrow_mut();
            let shown = revealed.note_shown(&note_id);
            *revealed.replaced.entry(id.clone()).or_insert(shown)
        };
        if shown <= 0.0 || !crate::motion::enabled() || self.drawing_leaving.get() {
            return content;
        }
        let note = note.absolute().top_0().left_0().right_0().with_animation(
            SharedString::from(format!("replaced-{id}")),
            Animation::new(APPEAR.time()).with_easing(crate::motion::ease_in_out),
            move |note, t| note.opacity(shown * (1.0 - t)),
        );
        div()
            .relative()
            .child(content)
            .child(note)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_wait_then_fade_in() {
        assert_eq!(note_opacity(0.1, 0.4, 0.3), 0.0);
        assert_eq!(note_opacity(0.4, 0.4, 0.3), 0.0);
        assert!(note_opacity(0.55, 0.4, 0.3) > 0.0);
        assert_eq!(note_opacity(0.8, 0.4, 0.3), 1.0);
    }

    #[test]
    fn only_what_comes_later_fades_in() {
        let mut revealed = Revealed::default();
        revealed.page_opened();
        let early: SharedString = "early".into();
        let later: SharedString = "later".into();
        assert!(!revealed.late(&early));
        std::thread::sleep(WITH_THE_PAGE * 2);
        assert!(revealed.late(&later));
        // Decided once: still not fading.
        assert!(!revealed.late(&early));
        // A new page starts over.
        revealed.page_opened();
        assert!(!revealed.late(&later));
    }
}
