//! Playback reports to Jellyfin. They go out one at a time, in the order
//! they were made: sent in parallel, a late progress report could land after
//! "stopped" and put back a resume point on an item that was just finished.
//! Other work that must keep its place among them queues here too.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use anyhow::Result;
use gpui::Context;

use super::{Library, Source, outbox::Change};
use crate::jellyfin::{self, PlaySession, PlaybackReport};

/// How long closing the window waits for unsent reports.
const CLOSE_WAIT: Duration = Duration::from_secs(4);
/// How often a closing window looks whether the reports are out.
const CLOSE_CHECK: Duration = Duration::from_millis(50);

type Send = Box<dyn FnOnce() -> Result<()> + std::marker::Send>;

/// What to tell a server about the video playing.
#[derive(Debug, Clone)]
pub struct Report {
    pub kind: PlaybackReport,
    /// Seconds into the video.
    pub position: f64,
    pub paused: bool,
    pub session: PlaySession,
}

/// What follows a report once the server has it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterReport {
    Nothing,
    /// The item's play state is read back: what the server made of it.
    ReadPlayState,
}

/// An item on a signed-in server (`server` is our row id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteItem {
    pub server: i64,
    pub id: String,
}

/// What the queue knows about a job, besides how to do it.
#[derive(Debug)]
struct About {
    /// For the log, if it fails.
    label: &'static str,
    /// A progress report, superseded by any newer one.
    progress: bool,
    /// The item whose play state is read back afterwards.
    confirm: Option<RemoteItem>,
    /// The change it carries, in the outbox until the server has it.
    remembered: Option<u64>,
    /// It needs the server: its outcome says if the server can be reached.
    report: bool,
}

struct Job {
    about: About,
    send: Send,
}

/// The reports and tasks still to send, in order.
#[derive(Default)]
pub(super) struct Reports {
    jobs: VecDeque<Job>,
    /// Something is sending them, one after the other.
    running: bool,
    /// The window is closing once the queue is empty.
    closing: bool,
    /// The last report found no connection (more aren't logged one by one).
    failing: bool,
}

impl Reports {
    pub(super) fn is_failing(&self) -> bool {
        self.failing
    }

    fn busy(&self) -> bool {
        self.running || !self.jobs.is_empty()
    }

    /// Adds a job at the end. Only the newest position matters, so a
    /// progress report takes the place of older unsent ones.
    fn push(&mut self, job: Job) {
        if job.about.progress {
            self.jobs.retain(|waiting| !waiting.about.progress);
        }
        self.jobs.push_back(job);
    }

    /// The next job to send, if any; with none left, sending has stopped.
    fn next(&mut self) -> Option<Job> {
        let next = self.jobs.pop_front();
        self.running = next.is_some();
        next
    }
}

impl Library {
    /// Tells the server about playback so its resume/watched state follows.
    /// A "stopped" that's read back is kept until the server has it.
    pub fn report(
        &mut self,
        source: &Source,
        report: Report,
        after: AfterReport,
        cx: &mut Context<Self>,
    ) {
        let Source::Jellyfin { server, id } = source else {
            return;
        };
        let Some(client) = self.client(*server) else {
            return;
        };
        let Report {
            kind,
            position,
            paused,
            session,
        } = report;
        let confirm = (after == AfterReport::ReadPlayState).then(|| RemoteItem {
            server: *server,
            id: id.clone(),
        });
        // Where it stopped mustn't be lost if the server can't be reached.
        let remembered = (kind == PlaybackReport::Stopped && confirm.is_some())
            .then(|| self.remember(*server, id, Change::StoppedAt(position)));
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
        let about = About {
            label: match kind {
                PlaybackReport::Started => "report Started",
                PlaybackReport::Progress => "report Progress",
                PlaybackReport::Stopped => "report Stopped",
            },
            progress: kind == PlaybackReport::Progress,
            confirm,
            remembered,
            report: true,
        };
        self.enqueue(Job { about, send }, cx);
    }

    /// Queues other work that belongs with the reports, after those
    /// already queued (closing the window waits for it too).
    pub fn queue_task(
        &mut self,
        label: &'static str,
        send: impl FnOnce() -> Result<()> + std::marker::Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let about = About {
            label,
            progress: false,
            confirm: None,
            remembered: None,
            report: false,
        };
        let send = Box::new(send);
        self.enqueue(Job { about, send }, cx);
    }

    /// Like `queue_task`, for work that tells the server of `change` to
    /// `item`: kept until the server has it, then the item is read back.
    pub fn queue_checked(
        &mut self,
        label: &'static str,
        item: RemoteItem,
        change: Change,
        send: impl FnOnce() -> Result<()> + std::marker::Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let about = About {
            label,
            progress: false,
            remembered: Some(self.remember(item.server, &item.id, change)),
            confirm: Some(item),
            report: true,
        };
        let send = Box::new(send);
        self.enqueue(Job { about, send }, cx);
    }

    fn enqueue(&mut self, job: Job, cx: &mut Context<Self>) {
        self.reports.push(job);
        if self.reports.running {
            return;
        }
        self.reports.running = true;
        cx.spawn(async move |this, cx| {
            while let Ok(Some(job)) = this.update(cx, |this, _| this.reports.next()) {
                let Job { about, send } = job;
                let result = cx.background_executor().spawn(async move { send() }).await;
                let _ = this.update(cx, |this, cx| this.job_finished(about, &result, cx));
            }
        })
        .detach();
    }

    /// Takes in how a job went: what it carried is crossed off or kept,
    /// and whether the server answers is noted.
    fn job_finished(&mut self, about: About, result: &Result<()>, cx: &mut Context<Self>) {
        let error = match result {
            Ok(()) => return self.job_sent(about, cx),
            Err(error) => error,
        };
        let offline = jellyfin::is_offline(error);
        // While offline, a progress report fails every few seconds: say so
        // once.
        if !(offline && about.progress && self.reports.failing) {
            eprintln!("[jellyfin] {}: {error:#}", about.label);
        }
        if about.report {
            self.reports.failing = offline;
        }
        if let Some(sequence) = about.remembered {
            self.sending_failed(sequence, error, cx);
        }
    }

    fn job_sent(&mut self, about: About, cx: &mut Context<Self>) {
        if let Some(sequence) = about.remembered {
            self.forget(sequence);
        }
        if about.report && std::mem::take(&mut self.reports.failing) {
            eprintln!("[jellyfin] the server answers again");
            self.server_back(cx);
        }
        if let Some(item) = about.confirm
            && !self.reports.closing
        {
            self.confirm_play_state(item.server, item.id, cx);
        }
    }

    /// Whether the app is only waiting for its last reports to go out.
    pub fn is_closing(&self) -> bool {
        self.reports.closing
    }

    /// The window is closing: quits the app once the reports are out, or
    /// after a short wait. True if nothing is waiting, to close at once.
    pub fn close_when_sent(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.reports.busy() {
            return true;
        }
        self.reports.closing = true;
        cx.notify();
        let deadline = Instant::now() + CLOSE_WAIT;
        cx.spawn(async move |this, cx| {
            while Instant::now() < deadline {
                cx.background_executor().timer(CLOSE_CHECK).await;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn job(label: &'static str, kind: PlaybackReport) -> Job {
        Job {
            about: About {
                label,
                progress: kind == PlaybackReport::Progress,
                confirm: None,
                remembered: None,
                report: true,
            },
            send: Box::new(|| Ok(())),
        }
    }

    fn waiting(reports: &Reports) -> Vec<&'static str> {
        reports.jobs.iter().map(|job| job.about.label).collect()
    }

    #[test]
    fn a_newer_progress_report_takes_the_place_of_an_unsent_one() {
        let mut reports = Reports::default();
        reports.push(job("started", PlaybackReport::Started));
        reports.push(job("at 10 s", PlaybackReport::Progress));
        reports.push(job("at 20 s", PlaybackReport::Progress));
        assert_eq!(waiting(&reports), ["started", "at 20 s"]);
    }

    #[test]
    fn started_and_stopped_reports_all_go_out_in_the_order_made() {
        let mut reports = Reports::default();
        reports.push(job("at 10 s", PlaybackReport::Progress));
        reports.push(job("stopped", PlaybackReport::Stopped));
        reports.push(job("started", PlaybackReport::Started));
        reports.push(job("stopped again", PlaybackReport::Stopped));
        assert_eq!(
            waiting(&reports),
            ["at 10 s", "stopped", "started", "stopped again"]
        );
    }

    #[test]
    fn a_progress_report_made_after_stopped_goes_out_after_it() {
        let mut reports = Reports::default();
        reports.push(job("at 10 s", PlaybackReport::Progress));
        reports.push(job("stopped", PlaybackReport::Stopped));
        reports.push(job("at 5 s of the next", PlaybackReport::Progress));
        assert_eq!(waiting(&reports), ["stopped", "at 5 s of the next"]);
    }

    #[test]
    fn the_queue_is_busy_until_its_last_job_has_been_taken_and_done() {
        let mut reports = Reports::default();
        assert!(!reports.busy());
        reports.push(job("stopped", PlaybackReport::Stopped));
        assert!(reports.busy());

        let taken = reports.next();
        assert_eq!(taken.map(|job| job.about.label), Some("stopped"));
        // Still being sent.
        assert!(reports.busy());
        assert!(reports.next().is_none());
        assert!(!reports.busy());
    }
}
