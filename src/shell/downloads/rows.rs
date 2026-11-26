//! The lines of the Downloads page and of an app's Activity tab: a download
//! with its progress, something that recently happened, and the sections
//! listing them. Drawn for whichever view shows them; a click on a line
//! goes back to that view.

use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, FontWeight, SharedString, div, prelude::*, px,
    relative, rgb, rgba,
};

use crate::arr::connections::Id;
use crate::arr::downloads::{
    self, AppFeed, Downloads, Entry, Feed, Feeds, Quiet, Target, TitleIndex,
};
use crate::arr::{self, Download, DownloadState, Event, Kind, Recent, Subject};
use crate::library;
use crate::shell::{ACCENT, ERROR, MUTED, TEXT, widgets};

/// The colour of "Added" on a history line.
const ADDED: u32 = 0x7ad89a;

/// What the drawing view does when a line that leads somewhere is clicked.
pub(in crate::shell) type Open<V> = fn(&mut V, &Target, &mut Context<V>);

/// What the apps are doing as of one draw, and the library's titles.
pub(in crate::shell) struct Activity {
    feeds: Rc<Feeds>,
    titles: Rc<TitleIndex>,
    /// Unix seconds, for "3 min ago".
    now: i64,
}

/// One "Downloading" and "Recently" pair: an app's, or a server's apps'.
struct Listing<'a> {
    server: i64,
    feeds: &'a [AppFeed<'a>],
    recent: Vec<Entry<'a>>,
    /// Who is being asked, for "Asking Sonarr and Radarr…".
    asking: String,
}

impl Activity {
    pub(in crate::shell) fn of(downloads: &Entity<Downloads>, cx: &App) -> Self {
        let downloads = downloads.read(cx);
        Self {
            feeds: downloads.feeds(),
            titles: downloads.titles(cx),
            now: library::now(),
        }
    }

    /// One app's Downloading and Recently, for its page.
    pub(in crate::shell) fn app_sections<V: 'static>(
        &self,
        id: Id,
        cx: &mut Context<V>,
        open: Open<V>,
    ) -> Vec<AnyElement> {
        let kind = id.kind;
        let feeds: Vec<AppFeed<'_>> = self
            .feeds
            .feed(id)
            .map(|feed| AppFeed { kind, feed })
            .into_iter()
            .collect();
        let recent: Vec<Entry<'_>> = feeds
            .iter()
            .flat_map(|app| app.feed.recent_shown())
            .map(|recent| Entry { kind, recent })
            .collect();
        let listing = Listing {
            server: id.server,
            feeds: &feeds,
            recent,
            asking: kind.name().to_string(),
        };
        self.sections(listing, cx, open)
    }

    /// A server's Downloading and Recently: its apps' (`kinds`) together.
    pub(in crate::shell) fn server_sections<V: 'static>(
        &self,
        server: i64,
        kinds: &[Kind],
        cx: &mut Context<V>,
        open: Open<V>,
    ) -> Vec<AnyElement> {
        let feeds = self.feeds.of_server(server, kinds);
        let names = kinds
            .iter()
            .map(|k| k.name())
            .collect::<Vec<_>>()
            .join(" and ");
        let listing = Listing {
            server,
            feeds: &feeds,
            recent: downloads::latest(&feeds),
            asking: names,
        };
        self.sections(listing, cx, open)
    }

    fn sections<V: 'static>(
        &self,
        listing: Listing<'_>,
        cx: &mut Context<V>,
        open: Open<V>,
    ) -> Vec<AnyElement> {
        let Listing {
            server,
            feeds,
            recent,
            asking,
        } = listing;
        let mut downloading: Vec<AnyElement> = feeds
            .iter()
            .filter_map(|app| app.feed.error())
            .map(|error| note(error.to_string(), ERROR))
            .collect();
        for app in feeds {
            let id = Id {
                server,
                kind: app.kind,
            };
            for download in app.feed.downloads() {
                downloading.push(self.download_row(id, download, cx, open));
            }
        }
        let answered: Vec<&Feed> = feeds.iter().map(|app| app.feed).collect();
        match downloads::quiet(&answered) {
            Some(Quiet::Asking) => downloading.push(note(format!("Asking {asking}…"), MUTED)),
            Some(Quiet::Nothing) => {
                downloading.push(note("Nothing is downloading right now.".into(), MUTED));
            }
            None => {}
        }
        let mut sections = vec![widgets::section(
            "Downloading",
            div().flex().flex_col().gap_2().children(downloading),
        )];
        let rows: Vec<AnyElement> = recent
            .iter()
            .map(|entry| {
                let id = Id {
                    server,
                    kind: entry.kind,
                };
                self.recent_row(id, entry.recent, cx, open)
            })
            .collect();
        if !rows.is_empty() {
            sections.push(widgets::section(
                "Recently",
                div().flex().flex_col().gap_2().children(rows),
            ));
        }
        sections
    }

    fn download_row<V: 'static>(
        &self,
        id: Id,
        download: &Download,
        cx: &mut Context<V>,
        open: Open<V>,
    ) -> AnyElement {
        let state = download.state;
        let color = if state.is_problem() {
            ERROR
        } else if state == DownloadState::Downloading {
            TEXT
        } else {
            MUTED
        };
        let mut facts: Vec<String> = Vec::new();
        facts.extend(download.quality.clone());
        if download.size > 0.0 {
            let done = (download.size - download.sizeleft).max(0.0);
            facts.push(if download.sizeleft > 0.0 && done > 0.0 {
                format!("{} of {}", size(done), size(download.size))
            } else {
                size(download.size)
            });
        }
        if state == DownloadState::Downloading {
            facts.extend(download.timeleft.filter(|s| *s > 0).map(left));
        }
        facts.extend(download.client.clone());
        let progress = download.progress().unwrap_or(0.0);
        let bar = div()
            .h(px(4.0))
            .w_full()
            .rounded_full()
            .bg(rgba(0xffffff14))
            .child(
                div()
                    .h_full()
                    .w(relative(progress))
                    .rounded_full()
                    .bg(rgb(if state.is_problem() { ERROR } else { ACCENT })),
            );
        let body = div()
            .w_full()
            .flex()
            .flex_col()
            .gap_1p5()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(subject_line(&download.subject).flex_1())
                    .child(div().flex_none().text_sm().text_color(rgb(color)).child(
                        match download.progress() {
                            Some(p) if state == DownloadState::Downloading => {
                                format!("{} · {:.0}%", state.label(), p * 100.0)
                            }
                            _ => state.label().to_string(),
                        },
                    )),
            )
            .child(bar)
            // In a row: truncated text in a column is measured as nothing.
            .child(
                div().flex().child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .truncate()
                        .child(download.release.clone()),
                ),
            )
            .when(!facts.is_empty(), |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child(facts.join(" · ")),
                )
            })
            .when(!download.messages.is_empty(), |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(rgb(ERROR))
                        .child(download.messages.join(" · ")),
                )
            });
        let row = Row {
            id: format!("download-{}-{}-{}", id.server, id.kind.name(), download.id),
            target: self.titles.target(id.server, &download.subject),
        };
        activity_row(row, body, cx, open)
    }

    fn recent_row<V: 'static>(
        &self,
        id: Id,
        recent: &Recent,
        cx: &mut Context<V>,
        open: Open<V>,
    ) -> AnyElement {
        let color = match recent.event {
            Event::Imported => ADDED,
            Event::Failed => ERROR,
            Event::Grabbed => MUTED,
        };
        let mut facts: Vec<String> = Vec::new();
        facts.extend(recent.quality.clone());
        facts.push(ago(recent.at, self.now));
        let body = div()
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .flex_none()
                    .w(px(64.0))
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(color))
                    .child(recent.event.label()),
            )
            .child(subject_line(&recent.subject).flex_1())
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child(facts.join(" · ")),
            );
        let row = Row {
            id: format!("recent-{}-{}-{}", id.server, id.kind.name(), recent.id),
            target: self.titles.target(id.server, &recent.subject),
        };
        activity_row(row, body, cx, open)
    }
}

/// A line of small text in place of a list, or above it.
fn note(text: String, color: u32) -> AnyElement {
    div()
        .text_sm()
        .text_color(rgb(color))
        .child(text)
        .into_any_element()
}

/// A line's element id and where it leads in the library, if anywhere.
struct Row {
    id: String,
    target: Option<Target>,
}

/// A line's frame: clickable when it leads somewhere in the library.
fn activity_row<V: 'static>(
    row: Row,
    body: impl IntoElement,
    cx: &mut Context<V>,
    open: Open<V>,
) -> AnyElement {
    div()
        .id(SharedString::from(row.id))
        .flex()
        .flex_col()
        .gap_1p5()
        .px_4()
        .py_3()
        .rounded_lg()
        .bg(rgba(0xffffff08))
        .child(body)
        .when_some(row.target, |row, target| {
            row.cursor_pointer()
                .hover(|s| s.bg(rgba(0xffffff12)))
                .on_click(cx.listener(move |view, _: &ClickEvent, _, cx| open(view, &target, cx)))
        })
        .into_any_element()
}

/// Title and detail on one line: "Shameless (US)  Season 11 · 12 episodes".
fn subject_line(subject: &Subject) -> gpui::Div {
    let (title, detail) = subject.label();
    div()
        .flex()
        .items_baseline()
        .gap_2()
        .min_w(px(0.0))
        .child(
            div()
                .flex_none()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .children(detail.map(|d| {
            div()
                .min_w(px(0.0))
                .truncate()
                .text_sm()
                .text_color(rgb(MUTED))
                .child(d)
        }))
}

/// `3 min ago`, `5 h ago`, `yesterday`.
fn ago(at: i64, now: i64) -> String {
    let secs = (now - at).max(0);
    match secs {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86_400 => format!("{} h ago", secs / 3600),
        86_400..172_800 => "yesterday".into(),
        _ if secs < 30 * 86_400 => format!("{} days ago", secs / 86_400),
        _ => arr::format_date(at),
    }
}

/// `12 min left`, `2 h 5 min left`.
fn left(secs: i64) -> String {
    match secs {
        ..60 => "less than a minute left".into(),
        60..3600 => format!("{} min left", secs / 60),
        3600..86_400 => format!("{} h {} min left", secs / 3600, secs % 3600 / 60),
        _ => format!("{} days left", secs / 86_400),
    }
}

/// `12.3 GB`, `700 MB`.
fn size(bytes: f64) -> String {
    if bytes >= 1e9 {
        format!("{:.1} GB", bytes / 1e9)
    } else {
        format!("{:.0} MB", bytes / 1e6)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times() {
        assert_eq!(ago(1000, 1030), "just now");
        assert_eq!(ago(0, 7200), "2 h ago");
        assert_eq!(ago(0, 100_000), "yesterday");
        assert_eq!(left(754), "12 min left");
        assert_eq!(left(7384), "2 h 3 min left");
        assert_eq!(size(24_300_000_000.0), "24.3 GB");
    }
}
