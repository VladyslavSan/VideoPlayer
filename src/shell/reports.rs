//! Playback reports to Jellyfin. They go out one at a time, in the order
//! they were made: sent in parallel, a late progress report could land after
//! "stopped" and put back a resume point on an item that was just finished.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use anyhow::Result;
use gpui::Context;

use super::Shell;
use crate::jellyfin::{PlaySession, PlaybackReport};
use crate::library::Source;

/// How long closing the window waits for unsent reports.
const CLOSE_WAIT: Duration = Duration::from_secs(4);

type Send = Box<dyn FnOnce() -> Result<()> + std::marker::Send>;

struct Job {
    kind: PlaybackReport,
    send: Send,
    /// Re-read the library afterwards, so watched state shows up.
    refresh: bool,
}

#[derive(Default)]
pub struct Reports {
    jobs: VecDeque<Job>,
    running: bool,
    /// The window is closing once the queue is empty.
    closing: bool,
}

impl Reports {
    fn busy(&self) -> bool {
        self.running || !self.jobs.is_empty()
    }
}

impl Shell {
    /// Tells the server about playback so its resume/watched state follows.
    /// After "stopped", the library is re-read.
    pub(super) fn report(
        &mut self,
        source: &Source,
        kind: PlaybackReport,
        position: f64,
        session: Option<PlaySession>,
        cx: &mut Context<Self>,
    ) {
        self.queue_report(
            source,
            kind,
            position,
            session,
            kind == PlaybackReport::Stopped,
            cx,
        );
    }

    pub(super) fn queue_report(
        &mut self,
        source: &Source,
        kind: PlaybackReport,
        position: f64,
        session: Option<PlaySession>,
        refresh: bool,
        cx: &mut Context<Self>,
    ) {
        let (Source::Jellyfin { server, id }, Some(session)) = (source, session) else {
            return;
        };
        let Some(client) = self.client(*server) else {
            return;
        };
        let id = id.clone();
        let send: Send = Box::new(move || {
            let reported = client.report(kind, &id, position, false, &session);
            if kind == PlaybackReport::Stopped && session.transcoding {
                // Don't leave the server transcoding for nobody.
                if let Err(e) = client.stop_transcoding(&session) {
                    eprintln!("[jellyfin] stopping transcoding: {e:#}");
                }
            }
            reported
        });
        if kind == PlaybackReport::Progress {
            // Only the newest position matters; drop older unsent ones.
            self.reports
                .jobs
                .retain(|j| j.kind != PlaybackReport::Progress);
        }
        self.reports.jobs.push_back(Job {
            kind,
            send,
            refresh,
        });
        if self.reports.running {
            return;
        }
        self.reports.running = true;
        cx.spawn(async move |this, cx| {
            loop {
                let next = this.update(cx, |this, _| {
                    let next = this.reports.jobs.pop_front();
                    this.reports.running = next.is_some();
                    next
                });
                let Ok(Some(job)) = next else {
                    break;
                };
                let kind = job.kind;
                let send = job.send;
                if let Err(e) = cx.background_executor().spawn(async move { send() }).await {
                    eprintln!("[jellyfin] report {kind:?}: {e:#}");
                }
                if job.refresh {
                    let _ = this.update(cx, |this, cx| {
                        if !this.reports.closing {
                            this.refresh_remote(cx);
                        }
                    });
                }
            }
        })
        .detach();
    }

    /// The window's close button. Playback is finished as if the viewer had
    /// gone back to the library, and the window stays open (briefly) until
    /// the server has been told, so the resume point and watched state
    /// aren't lost. Returns whether to close right away.
    pub(super) fn close_requested(&mut self, cx: &mut Context<Self>) -> bool {
        if self.reports.closing {
            // Asked again while waiting: don't keep the user waiting.
            return true;
        }
        self.finish_playback(cx);
        if !self.reports.busy() {
            return true;
        }
        self.reports.closing = true;
        cx.notify();
        let deadline = Instant::now() + CLOSE_WAIT;
        cx.spawn(async move |this, cx| {
            while Instant::now() < deadline {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                if !this
                    .update(cx, |this, _| this.reports.busy())
                    .unwrap_or(false)
                {
                    break;
                }
            }
            let _ = cx.update(|cx| cx.quit());
        })
        .detach();
        false
    }
}
