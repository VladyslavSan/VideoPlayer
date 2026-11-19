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
    /// For the log, if it fails.
    label: &'static str,
    /// A progress report, superseded by any newer one.
    progress: bool,
    send: Send,
    /// Read this item's play state back afterwards (server, item id), so
    /// what the server made of it shows.
    confirm: Option<(i64, String)>,
    /// The change it carries, written down until the server has it (see
    /// `offline`).
    remembered: Option<u64>,
    /// A playback report (other tasks may not need the server at all), so
    /// whether it got through says whether the server can be reached.
    report: bool,
}

#[derive(Default)]
pub struct Reports {
    jobs: VecDeque<Job>,
    running: bool,
    /// The window is closing once the queue is empty.
    closing: bool,
    /// The last report failed for want of a connection (further failures
    /// aren't logged one by one).
    failing: bool,
}

impl Reports {
    pub(super) fn is_failing(&self) -> bool {
        self.failing
    }

    fn busy(&self) -> bool {
        self.running || !self.jobs.is_empty()
    }
}

impl Shell {
    /// Tells the server about playback so its resume/watched state follows.
    /// After "stopped", the item's state is read back.
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
            false,
            session,
            kind == PlaybackReport::Stopped,
            cx,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn queue_report(
        &mut self,
        source: &Source,
        kind: PlaybackReport,
        position: f64,
        paused: bool,
        session: Option<PlaySession>,
        confirm: bool,
        cx: &mut Context<Self>,
    ) {
        let (Source::Jellyfin { server, id }, Some(session)) = (source, session) else {
            return;
        };
        let Some(client) = self.client(*server) else {
            return;
        };
        let confirm = confirm.then(|| (*server, id.clone()));
        // Where it stopped mustn't be lost if the server can't be reached.
        let remembered = (kind == PlaybackReport::Stopped && confirm.is_some()).then(|| {
            self.remember(*server, id, super::offline::Change::StoppedAt(position))
                .seq
        });
        let id = id.clone();
        let send: Send = Box::new(move || {
            let reported = client.report(kind, &id, position, paused, &session);
            if kind == PlaybackReport::Stopped && session.transcoding {
                // Don't leave the server transcoding for nobody.
                if let Err(e) = client.stop_transcoding(&session) {
                    eprintln!("[jellyfin] stopping transcoding: {e:#}");
                }
            }
            reported
        });
        let progress = kind == PlaybackReport::Progress;
        if progress {
            // Only the newest position matters; drop older unsent ones.
            self.reports.jobs.retain(|j| !j.progress);
        }
        let label = match kind {
            PlaybackReport::Started => "report Started",
            PlaybackReport::Progress => "report Progress",
            PlaybackReport::Stopped => "report Stopped",
        };
        self.enqueue(
            Job {
                label,
                progress,
                send,
                confirm,
                remembered,
                report: true,
            },
            cx,
        );
    }

    /// Queues other work that belongs with the reports, after those
    /// already queued (closing the window waits for it too).
    pub(super) fn queue_task(
        &mut self,
        label: &'static str,
        send: impl FnOnce() -> Result<()> + std::marker::Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.enqueue(
            Job {
                label,
                progress: false,
                send: Box::new(send),
                confirm: None,
                remembered: None,
                report: false,
            },
            cx,
        );
    }

    /// Like `queue_task`, then reads the item's play state back. `change`
    /// is written down until the server has it.
    pub(super) fn queue_checked(
        &mut self,
        label: &'static str,
        send: impl FnOnce() -> Result<()> + std::marker::Send + 'static,
        item: (i64, String),
        change: super::offline::Change,
        cx: &mut Context<Self>,
    ) {
        let remembered = Some(self.remember(item.0, &item.1, change).seq);
        self.enqueue(
            Job {
                label,
                progress: false,
                send: Box::new(send),
                confirm: Some(item),
                remembered,
                report: true,
            },
            cx,
        );
    }

    fn enqueue(&mut self, job: Job, cx: &mut Context<Self>) {
        self.reports.jobs.push_back(job);
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
                let send = job.send;
                let result = cx.background_executor().spawn(async move { send() }).await;
                let failed = result.is_err();
                let _ = this.update(cx, |this, cx| match &result {
                    Ok(()) => {
                        if let Some(seq) = job.remembered {
                            this.forget(seq);
                        }
                        if job.report && std::mem::take(&mut this.reports.failing) {
                            eprintln!("[jellyfin] the server answers again");
                            this.server_back(cx);
                        }
                    }
                    Err(e) => {
                        let offline = crate::jellyfin::is_offline(e);
                        // While offline, a progress report fails every few
                        // seconds: say so once.
                        if !(offline && job.progress && this.reports.failing) {
                            eprintln!("[jellyfin] {}: {e:#}", job.label);
                        }
                        if job.report {
                            this.reports.failing = offline;
                        }
                        if let Some(seq) = job.remembered {
                            this.sending_failed(seq, e, cx);
                        }
                    }
                });
                if failed {
                    continue;
                }
                if let Some((server, id)) = job.confirm {
                    let _ = this.update(cx, |this, cx| {
                        if !this.reports.closing {
                            this.confirm_play_state(server, id, cx);
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
