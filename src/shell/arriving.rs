//! Things that arrive after their page opened (Sonarr's answer, an
//! overview, a list) fade in rather than pop; things already there when the
//! page opened just come with the page's own fade. A "loading" note fades
//! in only once the wait is long enough to notice, so a quick answer
//! never flashes one; if it did show, it fades out as what it waited for
//! fades in.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt, AnyElement, IntoElement, ParentElement, SharedString, Styled, div,
};

use super::Shell;
use crate::motion::{APPEAR, WAIT};

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
}

impl Revealed {
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
