//! Details pages: a movie's page, and the header of a show's page. The
//! server is asked for overview, genres, ratings, stream names and a large
//! backdrop when the page opens; local files show what the library knows.

use std::path::PathBuf;

use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, ObjectFit, SharedString, Window, div, img,
    linear_color_stop, linear_gradient, prelude::*, px, rgb, rgba,
};

use super::{BG, MUTED, Screen, Shell, TEXT};
use crate::jellyfin::{Client, ItemInfo};
use crate::library::{self, Item, Source};

/// Width of the backdrop downloaded for a details page.
const BACKDROP_WIDTH: u32 = 1920;
const HERO_HEIGHT: f32 = 420.0;

/// Extra information for a details page, fetched when it opens.
#[derive(Debug, Clone, Default)]
pub struct Details {
    pub overview: Option<String>,
    pub tagline: Option<String>,
    pub genres: Vec<String>,
    pub rating: Option<f32>,
    pub official_rating: Option<String>,
    pub video: Vec<String>,
    pub audio: Vec<String>,
    pub subtitles: Vec<String>,
    pub backdrop: Option<PathBuf>,
    /// The TVDB id, which links a show to Sonarr.
    pub tvdb: Option<String>,
}

impl Details {
    fn from_info(info: &ItemInfo, backdrop: Option<PathBuf>) -> Self {
        Self {
            overview: info.overview.clone().filter(|o| !o.trim().is_empty()),
            tagline: info.taglines.first().cloned(),
            genres: info.genres.clone(),
            rating: info.community_rating,
            official_rating: info.official_rating.clone(),
            video: info.streams("Video"),
            audio: info.streams("Audio"),
            subtitles: info.streams("Subtitle"),
            backdrop,
            tvdb: info
                .provider_ids
                .iter()
                .find(|(k, v)| k.eq_ignore_ascii_case("Tvdb") && !v.trim().is_empty())
                .map(|(_, v)| v.trim().to_string()),
        }
    }
}

/// `1 h 42 min`, or `42 min`.
pub fn format_runtime(seconds: f64) -> String {
    let minutes = ((seconds / 60.0).round() as u64).max(1);
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// Fetches the item's info, and its backdrop at full width (cached on disk).
fn fetch(
    client: &Client,
    user_id: &str,
    item_id: &str,
    thumbs: &std::path::Path,
) -> anyhow::Result<Details> {
    let info = client.item_info(user_id, item_id)?;
    let backdrop = info.backdrop().and_then(|image| {
        let path = image.cache_path_width(thumbs, BACKDROP_WIDTH);
        if !path.exists()
            && let Err(e) = client.download_image_width(&image, &path, BACKDROP_WIDTH)
        {
            eprintln!("[jellyfin] backdrop: {e:#}");
            return None;
        }
        Some(path)
    });
    Ok(Details::from_info(&info, backdrop))
}

impl Shell {
    pub(super) fn open_details(&mut self, key: String, cx: &mut Context<Self>) {
        self.screen = Screen::Details(key.clone());
        let item = self.items().find(|i| i.key() == key).cloned();
        if let Some(item) = item {
            self.load_details(key, item, false, cx);
        }
        cx.notify();
    }

    /// Loads details for an item (`for_show = false`) or for the show an
    /// episode belongs to, stored under `key`. Local files have no extra
    /// details, and each key is only fetched once per session.
    pub(super) fn load_details(
        &mut self,
        key: String,
        item: Item,
        for_show: bool,
        cx: &mut Context<Self>,
    ) {
        let Source::Jellyfin { server, id } = &item.source else {
            return;
        };
        if self.details.contains_key(&key) || !self.details_loading.insert(key.clone()) {
            return;
        }
        let (Some(client), Some(user_id)) = (
            self.client(*server),
            self.servers
                .iter()
                .find(|s| s.id == *server)
                .map(|s| s.user_id.clone()),
        ) else {
            self.details_loading.remove(&key);
            return;
        };
        let id = id.clone();
        let thumbs = self.paths.thumbs.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let target = if for_show {
                        // An episode knows its series; the series has the overview.
                        client.item_info(&user_id, &id)?.series_id.unwrap_or(id)
                    } else {
                        id
                    };
                    fetch(&client, &user_id, &target, &thumbs)
                })
                .await;
            this.update(cx, |this, cx| {
                this.details_loading.remove(&key);
                match result {
                    Ok(details) => {
                        this.details.insert(key.clone(), details);
                        // A show's page can now find it in Sonarr.
                        if let Some(show) = key.strip_prefix("show:") {
                            this.load_sonarr(show.to_string(), cx);
                        }
                    }
                    Err(e) => eprintln!("[jellyfin] details: {e:#}"),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Marks an item watched or unwatched, locally and on its server.
    fn set_watched(&mut self, key: &str, watched: bool, cx: &mut Context<Self>) {
        let Some(item) = self.items().find(|i| i.key() == key).cloned() else {
            return;
        };
        match &item.source {
            Source::Local { id, .. } => {
                if let Err(e) = self.db.set_watched(*id, watched) {
                    self.error = Some(format!("Couldn't update {}: {e}", item.title).into());
                }
                self.reload();
            }
            Source::Jellyfin { server, id } => {
                if let Some(remote) = self.remote.iter_mut().find(|i| i.key() == key) {
                    remote.watched = watched;
                    remote.position = 0.0;
                    if watched {
                        remote.last_played = Some(super::now());
                    }
                }
                let user_id = self
                    .servers
                    .iter()
                    .find(|s| s.id == *server)
                    .map(|s| s.user_id.clone());
                if let (Some(client), Some(user_id)) = (self.client(*server), user_id) {
                    let id = id.clone();
                    cx.background_executor()
                        .spawn(async move {
                            if let Err(e) = client.set_played(&user_id, &id, watched) {
                                eprintln!("[jellyfin] marking watched: {e:#}");
                            }
                        })
                        .detach();
                }
            }
        }
        cx.notify();
    }

    fn action_button(
        id: impl Into<gpui::ElementId>,
        label: impl Into<SharedString>,
        primary: bool,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        div()
            .id(id.into())
            .px_5()
            .py_2()
            .rounded_full()
            .font_weight(FontWeight::SEMIBOLD)
            .cursor_pointer()
            .bg(rgba(if primary { 0xf4f5f7ff } else { 0xffffff1f }))
            .text_color(rgb(if primary { 0x101218 } else { TEXT }))
            .hover(|s| s.opacity(0.85))
            .child(label.into())
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| action(this, window, cx)))
            .into_any_element()
    }

    /// A wide image with the title and facts over a fade to the background.
    fn hero(
        image: Option<PathBuf>,
        title: String,
        facts: Vec<String>,
        tagline: Option<String>,
    ) -> AnyElement {
        div()
            .relative()
            .w_full()
            .h(px(HERO_HEIGHT))
            .mt_2()
            .rounded_2xl()
            .overflow_hidden()
            .bg(rgb(0x1a1d26))
            .children(image.map(|path| img(path).size_full().object_fit(ObjectFit::Cover)))
            .child(div().absolute().inset_0().bg(linear_gradient(
                180.0,
                linear_color_stop(rgba(BG << 8), 0.3),
                linear_color_stop(rgba((BG << 8) | 0xf2), 1.0),
            )))
            .child(
                div()
                    .absolute()
                    .left_8()
                    .right_8()
                    .bottom_8()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_3xl().font_weight(FontWeight::BOLD).child(title))
                    .when(!facts.is_empty(), |d| {
                        d.child(
                            div()
                                .text_sm()
                                .text_color(rgb(0xc9ceda))
                                .child(facts.join("  ·  ")),
                        )
                    })
                    .children(
                        tagline.map(|t| div().text_sm().italic().text_color(rgb(MUTED)).child(t)),
                    ),
            )
            .into_any_element()
    }

    fn facts(
        item_year: Option<u16>,
        duration: Option<f64>,
        details: Option<&Details>,
    ) -> Vec<String> {
        let mut facts = Vec::new();
        facts.extend(item_year.map(|y| y.to_string()));
        facts.extend(duration.filter(|d| *d > 0.0).map(format_runtime));
        if let Some(d) = details {
            facts.extend(d.official_rating.clone());
            facts.extend(d.rating.map(|r| format!("★ {r:.1}")));
            if !d.genres.is_empty() {
                facts.push(
                    d.genres
                        .iter()
                        .take(3)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", "),
                );
            }
        }
        facts
    }

    fn overview(details: Option<&Details>, loading: bool) -> Option<AnyElement> {
        let text = match details.and_then(|d| d.overview.clone()) {
            Some(text) => text,
            None if loading => "Loading details…".into(),
            None => return None,
        };
        Some(
            div()
                .pt_6()
                .max_w(px(780.0))
                .line_height(px(24.0))
                .text_color(rgb(if loading { MUTED } else { 0xd5d9e3 }))
                .child(text)
                .into_any_element(),
        )
    }

    fn info_row(label: &str, values: Vec<String>) -> Option<AnyElement> {
        if values.is_empty() {
            return None;
        }
        Some(
            div()
                .flex()
                .gap_6()
                .child(
                    div()
                        .w(px(110.0))
                        .flex_none()
                        .text_sm()
                        .text_color(rgb(MUTED))
                        .child(label.to_string()),
                )
                .child(div().flex().flex_col().gap_1().text_sm().children(values))
                .into_any_element(),
        )
    }

    pub(super) fn details_page(&self, key: &str, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(item) = self.items().find(|i| i.key() == key) else {
            return vec![
                div()
                    .pt_10()
                    .text_color(rgb(MUTED))
                    .child("This item is no longer in the library.")
                    .into_any_element(),
            ];
        };
        let details = self.details.get(key);
        let loading = self.details_loading.contains(key);
        let image = details
            .and_then(|d| d.backdrop.clone())
            .or_else(|| item.thumb.clone());
        let mut page = vec![Self::hero(
            image,
            item.title.clone(),
            Self::facts(item.year, item.duration, details),
            details.and_then(|d| d.tagline.clone()),
        )];

        let play_key = key.to_string();
        let resume = item.progress().is_some();
        let play_label = match (resume, item.duration) {
            (true, Some(d)) => format!(
                "▶  Resume · {} left",
                format_runtime((d - item.position).max(0.0))
            ),
            (true, None) => "▶  Resume".into(),
            (false, _) => "▶  Play".into(),
        };
        let restart_key = key.to_string();
        let watched_key = key.to_string();
        let watched = item.watched;
        page.push(
            div()
                .flex()
                .items_center()
                .gap_3()
                .pt_5()
                .child(Self::action_button(
                    "details-play",
                    play_label,
                    true,
                    cx,
                    move |this, window, cx| this.play(&play_key, window, cx),
                ))
                .when(resume, |d| {
                    d.child(Self::action_button(
                        "details-restart",
                        "Play from beginning",
                        false,
                        cx,
                        move |this, window, cx| this.play_from_start(&restart_key, window, cx),
                    ))
                })
                .child(Self::action_button(
                    "details-watched",
                    if watched {
                        "Mark as unwatched"
                    } else {
                        "Mark as watched"
                    },
                    false,
                    cx,
                    move |this, _, cx| this.set_watched(&watched_key, !watched, cx),
                ))
                .into_any_element(),
        );
        page.extend(Self::overview(details, loading));

        let mut info = Vec::new();
        if let Some(d) = details {
            info.extend(Self::info_row("Video", d.video.clone()));
            info.extend(Self::info_row("Audio", d.audio.clone()));
            info.extend(Self::info_row("Subtitles", d.subtitles.clone()));
        }
        if let Source::Local { path, .. } = &item.source {
            info.extend(Self::info_row("File", vec![path.display().to_string()]));
        }
        if !info.is_empty() {
            page.push(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .pt_8()
                    .children(info)
                    .into_any_element(),
            );
        }
        page
    }

    /// The top of a show's page: backdrop, facts, overview, and a button for
    /// the episode to watch next.
    pub(super) fn show_header(&self, show: &str, cx: &mut Context<Self>) -> AnyElement {
        let key = format!("show:{show}");
        let details = self.details.get(&key);
        let loading = self.details_loading.contains(&key);
        let episodes: Vec<&Item> = self
            .items()
            .filter(|i| i.show.as_deref() == Some(show))
            .collect();
        let image = details
            .and_then(|d| d.backdrop.clone())
            .or_else(|| episodes.iter().find_map(|e| e.thumb.clone()));
        let mut facts = vec![format!("{} episodes", episodes.len())];
        facts.extend(Self::facts(None, None, details));

        // Next up if the show is being watched, else something in progress,
        // else the first unwatched episode, else the first one.
        let mut ordered = episodes.clone();
        ordered.sort_by_key(|e| e.watch_order());
        let next = library::next_up(ordered.iter().copied())
            .into_iter()
            .map(|(_, e)| e)
            .next()
            .or_else(|| {
                ordered
                    .iter()
                    .copied()
                    .filter(|e| e.progress().is_some())
                    .max_by_key(|e| e.last_played)
            })
            .or_else(|| ordered.iter().copied().find(|e| !e.watched))
            .or_else(|| ordered.first().copied());

        div()
            .flex()
            .flex_col()
            .child(Self::hero(image, show.to_string(), facts, None))
            .children(next.map(|episode| {
                let key = episode.key();
                let verb = if episode.progress().is_some() {
                    "Resume"
                } else {
                    "Play"
                };
                let label = match episode.episode_label() {
                    Some(l) => format!("▶  {verb} {l}"),
                    None => format!("▶  {verb} {}", episode.title),
                };
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .pt_5()
                    .child(Self::action_button(
                        "show-play",
                        label,
                        true,
                        cx,
                        move |this, window, cx| this.play(&key, window, cx),
                    ))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(MUTED))
                            .child(episode.title.clone()),
                    )
                    .into_any_element()
            }))
            .children(Self::overview(details, loading))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtimes() {
        assert_eq!(format_runtime(42.0 * 60.0), "42 min");
        assert_eq!(format_runtime(2.0 * 3600.0), "2 h");
        assert_eq!(format_runtime(6129.0), "1 h 42 min");
        assert_eq!(format_runtime(20.0), "1 min");
    }
}
