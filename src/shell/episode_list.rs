//! A season's episodes as a list, for a show Jellyfin doesn't have (yet):
//! what Sonarr knows of them (with whether each is downloaded), or, for a
//! show not added, what Sonarr's metadata service knows. Each line: its
//! number, title, air date and length, and a little of its overview.

use gpui::{AnyElement, FontWeight, SharedString, div, prelude::*, px, rgb, rgba};

use super::{MUTED, TEXT};
use crate::arr::{MetaEpisode, format_date, parse_utc};

/// Downloaded, in a line.
const HAVE: u32 = 0x7ad89a;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct EpisodeLine {
    pub number: u32,
    pub title: Option<String>,
    /// `2023-09-29`.
    pub air_date: Option<String>,
    pub runtime: Option<u32>,
    pub overview: Option<String>,
    /// Whether it's downloaded, when Sonarr has the show.
    pub downloaded: Option<bool>,
}

impl EpisodeLine {
    pub(super) fn from_meta(e: &MetaEpisode) -> Self {
        Self {
            number: e.number,
            title: e.title.clone(),
            air_date: e.air_date.clone(),
            runtime: e.runtime,
            overview: e.overview.clone(),
            downloaded: None,
        }
    }

    pub(super) fn from_sonarr(e: &crate::arr::Episode) -> Self {
        Self {
            number: e.episode_number,
            title: e.title.clone().filter(|t| !t.trim().is_empty()),
            air_date: e
                .air_date_utc
                .as_deref()
                .and_then(|d| d.get(..10).map(String::from)),
            runtime: e.runtime.filter(|m| *m > 0),
            overview: e.overview.clone().filter(|o| !o.trim().is_empty()),
            downloaded: Some(e.has_file),
        }
    }
}

/// `2023-09-29` as the app writes dates.
fn date(day: &str) -> Option<String> {
    parse_utc(&format!("{day}T00:00:00Z")).map(format_date)
}

/// The episodes, one line each.
pub(super) fn episode_rows(episodes: &[EpisodeLine]) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .children(episodes.iter().map(|e| {
            let mut facts: Vec<String> = Vec::new();
            facts.extend(e.air_date.as_deref().and_then(date));
            facts.extend(e.runtime.map(|m| format!("{m} min")));
            let title: SharedString = e
                .title
                .clone()
                .unwrap_or_else(|| format!("Episode {}", e.number))
                .into();
            div()
                .flex()
                .gap_4()
                .px_4()
                .py_2p5()
                .rounded_md()
                .bg(rgba(0xffffff05))
                .child(
                    div()
                        .w(px(32.0))
                        .flex_none()
                        .text_sm()
                        .text_color(rgb(MUTED))
                        .child(format!("{}", e.number)),
                )
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
                                .items_baseline()
                                .gap_3()
                                .child(
                                    div()
                                        .min_w(px(0.0))
                                        .truncate()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(rgb(TEXT))
                                        .child(title),
                                )
                                .when(!facts.is_empty(), |d| {
                                    d.child(
                                        div()
                                            .flex_none()
                                            .text_sm()
                                            .text_color(rgb(MUTED))
                                            .child(facts.join(" · ")),
                                    )
                                })
                                .children(e.downloaded.filter(|have| *have).map(|_| {
                                    div()
                                        .flex_none()
                                        .text_sm()
                                        .text_color(rgb(HAVE))
                                        .child("Downloaded")
                                })),
                        )
                        .children(e.overview.clone().map(|o| {
                            div()
                                .text_sm()
                                .text_color(rgb(MUTED))
                                .line_clamp(2)
                                .child(o)
                        })),
                )
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_read_as_the_app_writes_them() {
        assert!(date("2023-09-29").is_some());
        assert_eq!(date("not a date"), None);
    }
}
