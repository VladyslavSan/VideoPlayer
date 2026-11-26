//! The episodes panel: the show's seasons as tabs and the episodes of the
//! one picked.

use std::time::Instant;

use gpui::{
    AnyElement, ClickEvent, Context, Div, FontWeight, ObjectFit, Stateful, div, img, prelude::*,
    px, relative, rgb, rgba,
};

use super::widgets::{HOVER, light};
use super::{ACCENT, MUTED, season_name};
use crate::player::Player;
use crate::player::panels::{Dissolve, tab_light};
use crate::player::playing::Episode;

/// The tallest the episode list gets before it scrolls.
const LIST_MAX_HEIGHT: f32 = 340.0;
/// Behind an episode without a picture.
const THUMB_BACKGROUND: u32 = 0x2b3a55;
/// Behind the playing episode's row.
const PLAYING_ROW: u32 = 0x4c8dff2e;
/// What lies under the mouse on a row.
const ROW_HOVER: u32 = 0xffffff14;

/// What an episode's row says under its title.
fn episode_state(episode: &Episode, playing: bool) -> String {
    if playing {
        "Now playing".to_string()
    } else if episode.watched {
        "Watched".to_string()
    } else if let Some(progress) = episode.progress {
        format!("{:.0}% watched", progress * 100.0)
    } else {
        String::new()
    }
}

impl Player {
    /// A small 16:9 picture for an episode, with its progress along the bottom.
    pub(super) fn episode_thumb(episode: &Episode, width: f32) -> AnyElement {
        let label = episode.number.map(|n| n.to_string()).unwrap_or_default();
        div()
            .relative()
            .flex_none()
            .w(px(width))
            .h(px(width * 9.0 / 16.0))
            .rounded_md()
            .overflow_hidden()
            .bg(rgb(THUMB_BACKGROUND))
            .child(match &episode.thumb {
                Some(path) => img(path.clone())
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .into_any_element(),
                None => div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(MUTED))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(label)
                    .into_any_element(),
            })
            .children(episode.progress.filter(|_| !episode.watched).map(|p| {
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .h(px(3.0))
                    .bg(rgba(0x00000080))
                    .child(div().h_full().w(relative(p)).bg(rgb(ACCENT)))
            }))
            .into_any_element()
    }

    pub(super) fn episodes_panel(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let seasons = self.playing.seasons();
        // Switching seasons: the old list dissolves into the new one, and
        // the tab's highlight moves across.
        let dissolve = self.panels.dissolve();
        Self::panel_frame("episodes-panel")
            .flex()
            .flex_col()
            .gap_2()
            .when(seasons.len() > 1, |d| {
                let tabs = self.season_tabs(&seasons, dissolve, cx);
                d.child(div().flex().flex_wrap().gap_1().children(tabs))
            })
            .child(self.episode_list(dissolve, cx))
    }

    fn season_tabs(
        &self,
        seasons: &[u16],
        dissolve: Option<Dissolve>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let shown = self.panels.season();
        seasons
            .iter()
            .map(|&season| {
                let lit = tab_light(season, shown, dissolve);
                div()
                    .id(("season", season as usize))
                    .px_3()
                    .py_1()
                    .rounded_full()
                    .text_sm()
                    .cursor_pointer()
                    .when(lit > 0.0, |d| d.bg(light(lit)))
                    .hover(|d| d.bg(rgba(HOVER)))
                    .child(season_name(season))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.panels.switch_season(season, Instant::now());
                        this.poke(cx);
                    }))
                    .into_any_element()
            })
            .collect()
    }

    /// The shown season's episodes, with the season before fading out over
    /// them during a switch.
    fn episode_list(&self, dissolve: Option<Dissolve>, cx: &mut Context<Self>) -> AnyElement {
        let season = self.panels.season();
        let shown = div()
            .id(("episode-list", season as usize))
            .flex()
            .flex_col()
            .gap_0p5()
            .max_h(px(LIST_MAX_HEIGHT))
            .overflow_y_scroll()
            .children(self.episode_rows(season, cx));
        let Some(dissolve) = dissolve else {
            return shown.into_any_element();
        };
        div()
            .relative()
            .child(shown.opacity(dissolve.progress))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .max_h(px(LIST_MAX_HEIGHT))
                    .overflow_hidden()
                    .opacity(1.0 - dissolve.progress)
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .children(self.episode_rows(dissolve.from, cx)),
            )
            .into_any_element()
    }

    /// The episodes of `season`, for the episodes panel.
    fn episode_rows(&self, season: u16, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let current = self.playing.current;
        self.playing
            .episodes
            .iter()
            .enumerate()
            .filter(|(_, e)| e.season == season)
            .map(|(index, episode)| self.episode_row(index, episode, current == Some(index), cx))
            .collect()
    }

    fn episode_row(
        &self,
        index: usize,
        episode: &Episode,
        playing: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let title = match episode.number {
            Some(n) => format!("{n}. {}", episode.title),
            None => episode.title.clone(),
        };
        let key = episode.key.clone();
        div()
            .id(("episode", index))
            .flex()
            .items_center()
            .gap_3()
            .p_1p5()
            .rounded_lg()
            .cursor_pointer()
            .when(playing, |d| d.bg(rgba(PLAYING_ROW)))
            .hover(|d| d.bg(rgba(ROW_HOVER)))
            .child(Self::episode_thumb(episode, 112.0))
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(div().text_sm().child(title))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(if playing { ACCENT } else { MUTED }))
                            .child(episode_state(episode, playing)),
                    ),
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.open_episode(key.clone(), playing, cx);
            }))
            .into_any_element()
    }

    /// A click on an episode's row: plays it, or just closes the panel if
    /// it's the one playing.
    fn open_episode(&mut self, key: String, playing: bool, cx: &mut Context<Self>) {
        if playing {
            self.panels.close();
            self.poke(cx);
        } else {
            self.play_episode(key, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn episode(watched: bool, progress: Option<f32>) -> Episode {
        Episode {
            key: "key".into(),
            season: 1,
            number: Some(1),
            title: "Pilot".into(),
            watched,
            progress,
            thumb: None,
        }
    }

    #[test]
    fn a_row_says_how_far_its_episode_was_watched() {
        assert_eq!(episode_state(&episode(false, None), false), "");
        assert_eq!(
            episode_state(&episode(false, Some(0.42)), false),
            "42% watched"
        );
        assert_eq!(episode_state(&episode(true, Some(0.42)), false), "Watched");
    }

    #[test]
    fn the_playing_episode_says_so_whatever_its_progress() {
        assert_eq!(episode_state(&episode(true, None), true), "Now playing");
    }
}
