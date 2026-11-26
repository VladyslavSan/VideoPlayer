//! The "Up next" countdown: it starts in the end credits or when the video
//! ends, and plays the next episode when it runs out.

use std::time::{Duration, Instant};

use gpui::Context;

use super::{Player, TICK};

/// Seconds the "Up next" card counts down before playing the next episode.
const UP_NEXT_SECONDS: u64 = 8;

/// What to do once the video has ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AtEnd {
    /// Start the countdown to the next episode.
    Start,
    /// The countdown ran out.
    PlayNext,
    Wait,
    /// Nothing follows: leave the player.
    Leave,
}

/// What to do while the video plays, in or out of its credits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WhilePlaying {
    /// The credits began and an episode follows: start the countdown.
    Start,
    /// The countdown ran out during the credits.
    PlayNext,
    Wait,
    /// Out of the credits: no countdown, and "Watch credits" is forgotten.
    Reset,
}

/// The countdown to the next episode, while one runs.
pub(super) struct UpNext {
    /// When the next episode will start.
    at: Option<Instant>,
    /// "Watch credits" was chosen: no countdown until the video ends.
    credits_dismissed: bool,
}

impl UpNext {
    pub(super) fn new() -> Self {
        Self {
            at: None,
            credits_dismissed: false,
        }
    }

    pub(super) fn is_counting(&self) -> bool {
        self.at.is_some()
    }

    fn start(&mut self, now: Instant) {
        self.at = Some(now + Duration::from_secs(UP_NEXT_SECONDS));
    }

    /// Stops the countdown (a seek away, a reload, a lost connection).
    pub(super) fn cancel(&mut self) {
        self.at = None;
    }

    /// Out of the credits: no countdown, and "Watch credits" is forgotten.
    fn reset(&mut self) {
        self.at = None;
        self.credits_dismissed = false;
    }

    /// Dismisses the credits countdown and keeps watching.
    pub(super) fn watch_credits(&mut self) {
        self.credits_dismissed = true;
        self.at = None;
    }

    /// Puts the end off by `by`: the countdown waits while paused.
    fn hold(&mut self, by: Duration) {
        if let Some(at) = &mut self.at {
            *at += by;
        }
    }

    /// Seconds left on the countdown, while it runs.
    pub(super) fn remaining(&self, now: Instant) -> Option<u64> {
        let left = self.at?.saturating_duration_since(now);
        Some(left.as_secs_f64().ceil() as u64)
    }

    fn at_end(&self, has_next: bool, now: Instant) -> AtEnd {
        match (has_next, self.at) {
            (false, _) => AtEnd::Leave,
            (true, None) => AtEnd::Start,
            (true, Some(at)) if now >= at => AtEnd::PlayNext,
            (true, Some(_)) => AtEnd::Wait,
        }
    }

    fn while_playing(&self, in_credits: bool, has_next: bool, now: Instant) -> WhilePlaying {
        match (in_credits, has_next, self.at) {
            (true, true, None) if !self.credits_dismissed => WhilePlaying::Start,
            (true, true, Some(at)) if now >= at => WhilePlaying::PlayNext,
            (true, _, _) => WhilePlaying::Wait,
            // Seeking back out of the credits (or the end) cancels it.
            (false, _, _) => WhilePlaying::Reset,
        }
    }
}

impl Player {
    /// The video has ended: counts down to the next episode, or leaves.
    pub(super) fn follow_end(&mut self, cx: &mut Context<Self>) {
        // Report the full runtime so the item is marked watched.
        self.status.position = self.status.duration;
        let next = self.playing.next().map(|e| e.key.clone());
        match (self.up_next.at_end(next.is_some(), Instant::now()), next) {
            (AtEnd::Start, Some(key)) => {
                eprintln!("[player] ended; up next: {key} in {UP_NEXT_SECONDS} s");
                self.report(cx);
                self.up_next.start(Instant::now());
            }
            (AtEnd::PlayNext, Some(key)) => return self.play_episode(key, cx),
            (AtEnd::Leave, _) | (_, None) => return self.exit(cx),
            (AtEnd::Wait, Some(_)) => {}
        }
        cx.notify();
    }

    /// While the video plays: counts down through the end credits. True if
    /// the player left for the next episode.
    pub(super) fn follow_credits(&mut self, cx: &mut Context<Self>) -> bool {
        if self.status.paused {
            self.up_next.hold(TICK);
        }
        let in_credits = self.markers.in_credits(self.status.position);
        let next = self.playing.next().map(|e| e.key.clone());
        let step = self
            .up_next
            .while_playing(in_credits, next.is_some(), Instant::now());
        match step {
            WhilePlaying::Start => {
                let key = next.as_deref().unwrap_or_default();
                eprintln!("[player] credits; up next: {key} in {UP_NEXT_SECONDS} s");
                self.up_next.start(Instant::now());
            }
            WhilePlaying::PlayNext => {
                self.play_next(cx);
                return true;
            }
            WhilePlaying::Wait => {}
            WhilePlaying::Reset => self.up_next.reset(),
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counting(from: Instant) -> UpNext {
        let mut up_next = UpNext::new();
        up_next.start(from);
        up_next
    }

    #[test]
    fn the_end_starts_a_countdown_when_an_episode_follows() {
        let now = Instant::now();
        assert_eq!(UpNext::new().at_end(true, now), AtEnd::Start);
        assert_eq!(UpNext::new().at_end(false, now), AtEnd::Leave);
    }

    #[test]
    fn the_next_episode_plays_when_the_countdown_runs_out() {
        let start = Instant::now();
        let up_next = counting(start);
        assert_eq!(
            up_next.at_end(true, start + Duration::from_secs(7)),
            AtEnd::Wait
        );
        assert_eq!(
            up_next.at_end(true, start + Duration::from_secs(8)),
            AtEnd::PlayNext
        );
    }

    #[test]
    fn credits_start_a_countdown_unless_dismissed() {
        let now = Instant::now();
        let mut up_next = UpNext::new();
        assert_eq!(up_next.while_playing(true, true, now), WhilePlaying::Start);
        assert_eq!(up_next.while_playing(true, false, now), WhilePlaying::Wait);
        up_next.watch_credits();
        assert_eq!(up_next.while_playing(true, true, now), WhilePlaying::Wait);
    }

    #[test]
    fn watching_the_credits_stops_the_countdown() {
        let mut up_next = counting(Instant::now());
        up_next.watch_credits();
        assert!(!up_next.is_counting());
    }

    #[test]
    fn leaving_the_credits_resets_the_countdown() {
        let start = Instant::now();
        let up_next = counting(start);
        assert_eq!(
            up_next.while_playing(false, true, start),
            WhilePlaying::Reset
        );
        assert_eq!(
            up_next.while_playing(true, true, start + Duration::from_secs(9)),
            WhilePlaying::PlayNext
        );
    }

    #[test]
    fn the_countdown_waits_while_paused() {
        let start = Instant::now();
        let mut up_next = counting(start);
        up_next.hold(Duration::from_secs(2));
        assert_eq!(up_next.remaining(start), Some(10));
        assert_eq!(
            up_next.at_end(true, start + Duration::from_secs(9)),
            AtEnd::Wait
        );
    }

    #[test]
    fn the_seconds_left_round_up() {
        let start = Instant::now();
        let up_next = counting(start);
        assert_eq!(up_next.remaining(start), Some(8));
        assert_eq!(
            up_next.remaining(start + Duration::from_millis(7100)),
            Some(1)
        );
        assert_eq!(up_next.remaining(start + Duration::from_secs(20)), Some(0));
        assert_eq!(UpNext::new().remaining(start), None);
    }
}
