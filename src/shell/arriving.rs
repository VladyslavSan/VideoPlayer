//! Things that arrive after their page opened (Sonarr's answer, an
//! overview, a list) fade in rather than pop; things already there when the
//! page opened just come with the page's own fade. A "loading" note fades
//! in only once the wait is long enough to notice, so a quick answer
//! never flashes one.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{Animation, AnimationExt, AnyElement, IntoElement, SharedString, Styled};

use super::Shell;
use crate::motion::{APPEAR, WAIT};

/// Shown this soon after its page opened, it came with the page.
const WITH_THE_PAGE: Duration = Duration::from_millis(30);

/// What each page has shown so far: whether each thing fades in.
#[derive(Default)]
pub(super) struct Revealed {
    since: Option<Instant>,
    seen: HashMap<SharedString, bool>,
}

impl Revealed {
    /// A new page: everything on it is new.
    pub(super) fn page_opened(&mut self) {
        self.since = Some(Instant::now());
        self.seen.clear();
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

impl Shell {
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
                Animation::new(APPEAR.time()).with_easing(crate::motion::ease_out),
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
        let delay = WAIT.time().as_secs_f32();
        let fade = APPEAR.time().as_secs_f32();
        let total = delay + fade;
        let id = id.into();
        note.with_animation(
            SharedString::from(format!("waiting-{id}-{}", self.page_generation)),
            Animation::new(Duration::from_secs_f32(total)),
            move |note, t| {
                let shown = ((t * total - delay) / fade).clamp(0.0, 1.0);
                note.opacity(crate::motion::ease_out(shown))
            },
        )
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
