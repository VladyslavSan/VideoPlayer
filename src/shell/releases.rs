//! Choosing a release yourself, like Sonarr's interactive search: what the
//! indexers offer for a season, best first, Sonarr's own pick marked,
//! the rejected ones (with why) folded away, and a Download for each.

use std::time::Instant;

use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, SharedString, div, prelude::*, px, rgb, rgba,
};

use super::sonarr::{Request, SonarrView};
use super::{ACCENT, ERROR, MUTED, Shell, TEXT};
use crate::arr::Release;

/// Longest list drawn at once in each group.
const SHOWN: usize = 50;

pub(super) enum ReleaseList {
    Searching,
    Ready(Vec<Release>),
    Failed(SharedString),
}

pub(super) struct ReleasePanel {
    list: ReleaseList,
    show_rejected: bool,
    /// The release being sent, and the ones sent.
    grabbing: Option<String>,
    grabbed: Vec<String>,
    error: Option<SharedString>,
}

pub(super) type PanelKey = (String, u32);

/// `24.3 GB`, `700 MB`.
fn format_size(bytes: u64) -> String {
    let gb = bytes as f64 / 1e9;
    if gb >= 1.0 {
        format!("{gb:.1} GB")
    } else {
        format!("{:.0} MB", bytes as f64 / 1e6)
    }
}

/// `5 h`, `3 days`.
fn format_age(hours: f64) -> String {
    if hours < 48.0 {
        format!("{:.0} h", hours.max(1.0))
    } else {
        format!("{:.0} days", hours / 24.0)
    }
}

/// The line of facts under a release's name.
fn facts(release: &Release) -> String {
    let mut facts = Vec::new();
    facts.extend(release.quality());
    if release.full_season {
        facts.push("Season pack".into());
    }
    if release.size > 0 {
        facts.push(format_size(release.size));
    }
    if release.protocol == "torrent" {
        let seeders = release.seeders.unwrap_or(0);
        let peers = release.leechers.unwrap_or(0);
        facts.push(format!("{seeders} seeders, {peers} peers"));
    } else if !release.protocol.is_empty() {
        facts.push("Usenet".into());
    }
    facts.extend(release.age_hours.map(format_age));
    if !release.indexer.is_empty() {
        facts.push(release.indexer.clone());
    }
    let languages = release.languages();
    if !languages.is_empty() {
        facts.push(languages.join(", "));
    }
    if let Some(score) = release.custom_format_score.filter(|s| *s != 0) {
        facts.push(format!("score {score:+}"));
    }
    facts.join(" · ")
}

impl Shell {
    /// Opens the season's release list (searching the indexers), or
    /// closes it.
    pub(super) fn toggle_releases(&mut self, show: String, season: u32, cx: &mut Context<Self>) {
        let key = (show.clone(), season);
        if self.release_panels.remove(&key).is_some() {
            cx.notify();
            return;
        }
        let Some((client, _)) = self.sonarr_for(&show) else {
            return;
        };
        let Some(SonarrView::Ready(found)) = self.sonarr.get(&show) else {
            return;
        };
        let series = found.series;
        self.release_panels.insert(
            key.clone(),
            ReleasePanel {
                list: ReleaseList::Searching,
                show_rejected: false,
                grabbing: None,
                grabbed: Vec::new(),
                error: None,
            },
        );
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { client.season_releases(series, season) })
                .await;
            this.update(cx, |this, cx| {
                // Closed while searching: nothing to show.
                let Some(panel) = this.release_panels.get_mut(&key) else {
                    return;
                };
                panel.list = match result {
                    Ok(releases) => ReleaseList::Ready(releases),
                    Err(e) => {
                        eprintln!("[sonarr] releases for {} season {season}: {e:#}", key.0);
                        ReleaseList::Failed(format!("Couldn't search: {e:#}").into())
                    }
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn grab_release(&mut self, key: PanelKey, guid: String, cx: &mut Context<Self>) {
        let Some((client, _)) = self.sonarr_for(&key.0) else {
            return;
        };
        let Some(panel) = self.release_panels.get_mut(&key) else {
            return;
        };
        let ReleaseList::Ready(releases) = &panel.list else {
            return;
        };
        let Some(release) = releases.iter().find(|r| r.guid == guid).cloned() else {
            return;
        };
        panel.grabbing = Some(guid.clone());
        panel.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { client.grab(&release) })
                .await;
            this.update(cx, |this, cx| {
                if let Some(panel) = this.release_panels.get_mut(&key) {
                    panel.grabbing = None;
                    match &result {
                        Ok(()) => panel.grabbed.push(guid.clone()),
                        Err(e) => {
                            panel.error = Some(format!("Sonarr couldn't send it: {e:#}").into())
                        }
                    }
                }
                if result.is_ok() {
                    eprintln!("[sonarr] {} season {}: sent a chosen release", key.0, key.1);
                    this.sonarr_requests
                        .insert(key.clone(), Request::Grabbed(Instant::now()));
                    // Follows the download from Sonarr's queue.
                    this.load_sonarr(key.0.clone(), cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The release list under a season's line, when open.
    pub(super) fn release_panel(
        &self,
        show: &str,
        season: u32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let key: PanelKey = (show.to_string(), season);
        let panel = self.release_panels.get(&key)?;
        let note = |text: String, color: u32| div().text_sm().text_color(rgb(color)).child(text);
        let mut children: Vec<AnyElement> = Vec::new();
        match &panel.list {
            ReleaseList::Searching => children.push(
                note(
                    format!("Searching your indexers for season {season}… this can take a minute."),
                    MUTED,
                )
                .into_any_element(),
            ),
            ReleaseList::Failed(e) => children.push(note(e.to_string(), ERROR).into_any_element()),
            ReleaseList::Ready(releases) if releases.is_empty() => children.push(
                note("The indexers found nothing for this season.".into(), MUTED)
                    .into_any_element(),
            ),
            ReleaseList::Ready(releases) => {
                let approved: Vec<&Release> = releases.iter().filter(|r| r.approved).collect();
                let rejected: Vec<&Release> = releases.iter().filter(|r| !r.approved).collect();
                let summary = match (approved.len(), rejected.len()) {
                    (0, n) => format!("{n} releases found; Sonarr would reject all of them:"),
                    (a, 0) => format!("{a} releases found. The first is what Sonarr would pick."),
                    (a, n) => format!(
                        "{} releases found: {a} Sonarr would take (the first is its pick), {n} it would reject.",
                        a + n
                    ),
                };
                children.push(note(summary, MUTED).into_any_element());
                if let Some(error) = &panel.error {
                    children.push(note(error.to_string(), ERROR).into_any_element());
                }
                for (i, release) in approved.iter().take(SHOWN).enumerate() {
                    children.push(self.release_row(&key, panel, release, i == 0, cx));
                }
                let show_rejected = panel.show_rejected || approved.is_empty();
                if !rejected.is_empty() && !approved.is_empty() {
                    let label = if panel.show_rejected {
                        "Hide rejected releases".to_string()
                    } else {
                        format!("Show {} rejected releases", rejected.len())
                    };
                    let toggle = key.clone();
                    children.push(
                        div()
                            .id(SharedString::from(format!("rejected-{season}")))
                            .text_sm()
                            .text_color(rgb(MUTED))
                            .cursor_pointer()
                            .hover(|s| s.text_color(rgb(TEXT)))
                            .child(label)
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                if let Some(panel) = this.release_panels.get_mut(&toggle) {
                                    panel.show_rejected = !panel.show_rejected;
                                }
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
                if show_rejected {
                    for release in rejected.iter().take(SHOWN) {
                        children.push(self.release_row(&key, panel, release, false, cx));
                    }
                }
            }
        }
        Some(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .pl(px(24.0))
                .children(children)
                .into_any_element(),
        )
    }

    fn release_row(
        &self,
        key: &PanelKey,
        panel: &ReleasePanel,
        release: &Release,
        pick: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let sent = panel.grabbed.contains(&release.guid);
        let sending = panel.grabbing.as_deref() == Some(release.guid.as_str());
        let rejections = release.rejections();
        let action: AnyElement = if sent {
            div()
                .flex_none()
                .text_sm()
                .text_color(rgb(0x7ad89a))
                .child("Sent ✓")
                .into_any_element()
        } else if sending {
            div()
                .flex_none()
                .text_sm()
                .text_color(rgb(MUTED))
                .child("Sending…")
                .into_any_element()
        } else {
            let key = key.clone();
            let guid = release.guid.clone();
            self.button(
                SharedString::from(format!("grab-{}", release.guid)),
                "Download",
                cx,
                move |this, _, cx| this.grab_release(key.clone(), guid.clone(), cx),
            )
        };
        div()
            .flex()
            .items_center()
            .gap_4()
            .px_4()
            .py_2p5()
            .rounded_lg()
            .bg(rgba(if pick { 0x4c8dff1f } else { 0xffffff08 }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .min_w(px(0.0))
                            .children(pick.then(|| {
                                div()
                                    .flex_none()
                                    .px_2()
                                    .rounded_md()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .bg(rgb(ACCENT))
                                    .child("Sonarr's pick")
                            }))
                            .child(
                                div()
                                    .min_w(px(0.0))
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .truncate()
                                    .child(release.title.clone()),
                            ),
                    )
                    .child(div().text_xs().text_color(rgb(MUTED)).child(facts(release)))
                    .when(!rejections.is_empty(), |d| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(rgb(ERROR))
                                .child(rejections.join(" · ")),
                        )
                    }),
            )
            .child(action)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_ages() {
        assert_eq!(format_size(24_300_000_000), "24.3 GB");
        assert_eq!(format_size(700_000_000), "700 MB");
        assert_eq!(format_age(5.2), "5 h");
        assert_eq!(format_age(72.0), "3 days");
    }
}
