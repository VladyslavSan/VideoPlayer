//! What the open player tells its Jellyfin server: its position as it
//! plays, and that it's done. A player left paused here lets go of the item
//! after a while, and never writes its old position over what another
//! device did meanwhile. The library sends the reports (`library::reports`).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use gpui::Context;

use super::NowPlaying;
use crate::jellyfin::{Client, PlaySession, PlaybackReport, UserData};
use crate::library::{AfterReport, Change, RemoteItem, Report, Source};

/// A paused player tells the server it's done after this long, before the
/// server would end it by itself (5 minutes without a word).
const LET_GO_AFTER: Duration = Duration::from_secs(120);

/// A paused playback.
pub(super) struct Pause {
    since: Instant,
    /// The server's play state just after the pause; a later change was made elsewhere.
    baseline: Arc<Mutex<Option<UserData>>>,
    /// The server has been told this playback is over (it resumes as a new one).
    let_go: bool,
}

impl Pause {
    fn starting_now() -> Self {
        Self {
            since: Instant::now(),
            baseline: Arc::new(Mutex::new(None)),
            let_go: false,
        }
    }

    /// Whether this is still the pause begun at `since`, with the server
    /// not yet told it's over.
    fn waits_since(&self, since: Instant) -> bool {
        self.since == since && !self.let_go
    }

    /// A new stream carries the pause on from now.
    fn restart(&mut self) -> Instant {
        self.since = Instant::now();
        self.let_go = false;
        self.since
    }
}

/// What a progress report from the player means for its pause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Turn {
    /// Paused before and still: the server has heard.
    StillPaused,
    /// Just paused.
    Pauses,
    /// Playing; `after_let_go`: the server was told it was over meanwhile.
    Plays { after_let_go: bool },
}

impl Turn {
    fn of(paused: bool, pause: Option<&Pause>) -> Turn {
        match (paused, pause) {
            (true, Some(_)) => Turn::StillPaused,
            (true, None) => Turn::Pauses,
            (false, pause) => Turn::Plays {
                after_let_go: pause.is_some_and(|pause| pause.let_go),
            },
        }
    }
}

/// Why a paused playback is let go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reason {
    /// It stayed paused for `LET_GO_AFTER`.
    LongPause,
    /// The player is closing, so a transcode ends too.
    Closing,
}

/// Telling the server a paused playback is over, off the UI thread.
struct LetGo {
    client: Client,
    user_id: String,
    id: String,
    position: f64,
    session: PlaySession,
    baseline: Arc<Mutex<Option<UserData>>>,
    reason: Reason,
}

impl LetGo {
    /// Reports the paused position, unless the item was played elsewhere
    /// since the pause (then its play state stays).
    fn send(&self) -> Result<()> {
        let before = self.baseline.lock().unwrap().clone();
        let now = self.client.user_data(&self.user_id, &self.id).ok();
        let elsewhere =
            matches!((&before, &now), (Some(before), Some(now)) if now.moved_on_from(before));
        if elsewhere {
            eprintln!("[jellyfin] played elsewhere while paused here; its play state stays");
        }
        let reported = self
            .client
            .report_let_go(&self.id, self.position, &self.session, elsewhere);
        if self.reason == Reason::Closing
            && self.session.transcoding
            && let Err(e) = self.client.stop_transcoding(&self.session)
        {
            eprintln!("[jellyfin] stopping transcoding: {e:#}");
        }
        reported
    }
}

impl NowPlaying {
    /// Passes the player's position on to the server. Pausing is said
    /// once; playing again after the server was let go starts anew.
    pub(super) fn playback_progress(
        &mut self,
        position: f64,
        paused: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        let (Some(source), Some(session)) = (active.source.clone(), active.session.clone()) else {
            return;
        };
        let Source::Jellyfin { server, id } = source.clone() else {
            return;
        };
        let progress = Report {
            kind: PlaybackReport::Progress,
            position,
            paused,
            session: session.clone(),
        };
        match Turn::of(paused, active.pause.as_ref()) {
            Turn::StillPaused => {}
            Turn::Pauses => {
                let pause = Pause::starting_now();
                let (since, baseline) = (pause.since, pause.baseline.clone());
                active.pause = Some(pause);
                self.report(&source, progress, AfterReport::Nothing, cx);
                self.read_baseline(RemoteItem { server, id }, baseline, cx);
                self.let_go_later(since, cx);
            }
            Turn::Plays { after_let_go } => {
                active.pause = None;
                if after_let_go {
                    eprintln!("[jellyfin] playing again after letting go");
                    let started = Report {
                        kind: PlaybackReport::Started,
                        position,
                        paused: false,
                        session,
                    };
                    self.report(&source, started, AfterReport::Nothing, cx);
                }
                self.report(&source, progress, AfterReport::Nothing, cx);
            }
        }
    }

    /// Reads the item's play state on the server into `baseline`, after
    /// the reports queued so far.
    fn read_baseline(
        &mut self,
        item: RemoteItem,
        baseline: Arc<Mutex<Option<UserData>>>,
        cx: &mut Context<Self>,
    ) {
        let RemoteItem { server, id } = item;
        let (Some(client), Some(user_id)) = (self.client(server, cx), self.user_id(server, cx))
        else {
            return;
        };
        self.library.update(cx, |library, cx| {
            library.queue_task(
                "reading the play state",
                move || {
                    *baseline.lock().unwrap() = Some(client.user_data(&user_id, &id)?);
                    Ok(())
                },
                cx,
            )
        });
    }

    fn let_go_later(&mut self, since: Instant, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(LET_GO_AFTER).await;
            this.update(cx, |this, cx| {
                let pause = this.active.as_ref().and_then(|a| a.pause.as_ref());
                if pause.is_some_and(|pause| pause.waits_since(since)) {
                    this.let_go(Reason::LongPause, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Tells the server a paused playback is over, at the paused position,
    /// unless the item was played elsewhere since (then its state stays).
    fn let_go(&mut self, reason: Reason, cx: &mut Context<Self>) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        let (Some(Source::Jellyfin { server, id }), Some(session), Some(pause)) = (
            active.source.clone(),
            active.session.clone(),
            active.pause.as_mut(),
        ) else {
            return;
        };
        pause.let_go = true;
        let baseline = pause.baseline.clone();
        let position = active.view.read(cx).position();
        let (Some(client), Some(user_id)) = (self.client(server, cx), self.user_id(server, cx))
        else {
            return;
        };
        eprintln!("[jellyfin] letting go of the paused video at {position:.0} s");
        let let_go = LetGo {
            client,
            user_id,
            id: id.clone(),
            position,
            session,
            baseline,
            reason,
        };
        self.library.update(cx, |library, cx| {
            library.queue_checked(
                "letting go of the paused video",
                RemoteItem { server, id },
                Change::StoppedAt(position),
                move || let_go.send(),
                cx,
            )
        });
    }

    /// The player closed while paused, or after letting go: what the
    /// server needs to hear. False if it was playing (the usual report).
    pub(super) fn finish_paused(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(active) = self.active.as_ref() else {
            return false;
        };
        let (Some(Source::Jellyfin { server, id }), Some(session), Some(pause)) = (
            active.source.clone(),
            active.session.clone(),
            active.pause.as_ref(),
        ) else {
            return false;
        };
        if !pause.let_go {
            self.let_go(Reason::Closing, cx);
            return true;
        }
        let transcoder = self.client(server, cx).filter(|_| session.transcoding);
        self.library.update(cx, |library, cx| {
            if let Some(client) = transcoder {
                library.queue_task(
                    "ending the transcode",
                    move || client.stop_transcoding(&session),
                    cx,
                );
            }
            library.confirm_play_state(server, id, cx);
        });
        true
    }

    /// A new stream (another quality) restarts the server's playback: a
    /// pause carries on from now.
    pub(super) fn restart_pause(&mut self, cx: &mut Context<Self>) {
        let Some(pause) = self.active.as_mut().and_then(|a| a.pause.as_mut()) else {
            return;
        };
        let since = pause.restart();
        self.let_go_later(since, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pausing_is_said_once() {
        assert_eq!(Turn::of(true, None), Turn::Pauses);
        let pause = Pause::starting_now();
        assert_eq!(Turn::of(true, Some(&pause)), Turn::StillPaused);
    }

    #[test]
    fn playing_again_after_letting_go_starts_anew() {
        let mut pause = Pause::starting_now();
        let resumed = Turn::Plays {
            after_let_go: false,
        };
        assert_eq!(Turn::of(false, Some(&pause)), resumed);
        assert_eq!(Turn::of(false, None), resumed);
        pause.let_go = true;
        let anew = Turn::Plays { after_let_go: true };
        assert_eq!(Turn::of(false, Some(&pause)), anew);
    }

    #[test]
    fn a_pause_is_let_go_once_and_only_by_its_own_timer() {
        let mut pause = Pause::starting_now();
        let since = pause.since;
        assert!(pause.waits_since(since));
        assert!(!pause.waits_since(since + Duration::from_secs(1)));
        pause.let_go = true;
        assert!(!pause.waits_since(since));
    }

    #[test]
    fn a_new_stream_carries_the_pause_on() {
        let mut pause = Pause::starting_now();
        pause.let_go = true;
        let since = pause.restart();
        assert!(pause.waits_since(since));
    }
}
