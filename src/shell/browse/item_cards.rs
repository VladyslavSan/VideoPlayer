//! The cards of what the library holds: a movie's or an episode's, and a
//! show's. A click opens its page or plays it; a right click opens the
//! menu that marks it watched.

use gpui::{AnyElement, Context, SharedString};

use super::card_menu::MenuTarget;
use super::shelves::ShowSummary;
use super::{Pages, StartAt};
use crate::library::Item;
use crate::shell::cards::{Card, Clicks};

/// The id of an item's card among those of one row or grid (`prefix`).
pub(in crate::shell) fn item_card_id(prefix: &str, item: &Item) -> SharedString {
    format!("{prefix}-{}", item.key()).into()
}

/// The id of a show's card among those of one row or grid (`prefix`).
pub(in crate::shell) fn show_card_id(prefix: &str, show: &str) -> SharedString {
    format!("{prefix}-show-{show}").into()
}

/// How an item's card is titled, by where it is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Titled<'a> {
    /// On its own (Home, the grids, search): an episode leads with its show.
    WithShow,
    /// In its season: number and title. `ringed`: the episode the page opened at.
    InSeason { ringed: Option<&'a str> },
}

/// A card's title and the line under it.
fn card_text(item: &Item, titled: Titled<'_>) -> (String, Option<String>) {
    match (titled, &item.show, item.episode_label()) {
        (Titled::WithShow, Some(show), Some(label)) => {
            (show.clone(), Some(format!("{label} · {}", item.title)))
        }
        (Titled::InSeason { .. }, Some(_), _) => (
            format!("{}. {}", item.episode.unwrap_or(0), item.title),
            None,
        ),
        _ => (item.title.clone(), item.year.map(|y| y.to_string())),
    }
}

impl Pages {
    /// The card of a movie or an episode; `prefix` names its row or grid.
    pub(super) fn item_card(
        &self,
        prefix: &str,
        item: &Item,
        titled: Titled<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (title, subtitle) = card_text(item, titled);
        let key = item.key();
        let ringed = match titled {
            Titled::InSeason { ringed } => ringed == Some(key.as_str()),
            Titled::WithShow => false,
        };
        let card = Card {
            id: item_card_id(prefix, item),
            thumb: item.thumb.clone(),
            title,
            subtitle,
            progress: item.progress(),
            watched: item.watched,
            highlight: ringed,
        };
        let open_details = !item.is_episode() && prefix != "continue";
        let (click_key, menu_key) = (key.clone(), key.clone());
        let mut clicks = Clicks::new(move |this: &mut Self, _, cx| {
            if open_details {
                this.open_details(click_key.clone(), cx)
            } else {
                this.play(&click_key, StartAt::ResumePoint, cx)
            }
        })
        .on_menu(move |this, at, cx| {
            this.open_card_menu(MenuTarget::Item(menu_key.clone()), at, cx)
        });
        // An episode shown with its show's name (Continue Watching, search):
        // the name leads to the show, at this episode.
        if titled == Titled::WithShow && item.is_episode() {
            clicks = clicks.on_text(move |this, _, cx| this.open_show_at(&key, cx));
        }
        self.cards.card(card, clicks, cx)
    }

    /// A show's card; `prefix` names its row or grid.
    pub(super) fn show_card(
        &self,
        prefix: &str,
        show: &ShowSummary,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = show.name.clone();
        let card = Card {
            id: show_card_id(prefix, &show.name),
            thumb: show.thumb.clone(),
            title: show.name.clone(),
            subtitle: Some(show.subtitle()),
            progress: None,
            watched: show.unwatched == 0,
            highlight: false,
        };
        let menu_name = name.clone();
        let clicks = Clicks::new(move |this: &mut Self, _, cx| this.open_show(name.clone(), cx))
            .on_menu(move |this, at, cx| {
                this.open_card_menu(MenuTarget::Show(menu_name.clone()), at, cx)
            });
        self.cards.card(card, clicks, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Source;

    fn movie(title: &str, year: Option<u16>) -> Item {
        Item {
            library: None,
            source: Source::Jellyfin {
                server: 1,
                id: title.into(),
            },
            title: title.into(),
            year,
            show: None,
            season: None,
            episode: None,
            duration: None,
            position: 0.0,
            watched: false,
            last_played: None,
            thumb: None,
            added: None,
        }
    }

    fn episode() -> Item {
        Item {
            show: Some("Dark".into()),
            season: Some(1),
            episode: Some(4),
            ..movie("Double Lives", None)
        }
    }

    #[test]
    fn an_episode_on_its_own_leads_with_its_show() {
        let (title, subtitle) = card_text(&episode(), Titled::WithShow);
        assert_eq!(title, "Dark");
        assert_eq!(subtitle.as_deref(), Some("S1 · E4 · Double Lives"));
    }

    #[test]
    fn an_episode_in_its_season_is_numbered() {
        let (title, subtitle) = card_text(&episode(), Titled::InSeason { ringed: None });
        assert_eq!(title, "4. Double Lives");
        assert_eq!(subtitle, None);
    }

    #[test]
    fn a_movie_shows_its_title_and_year_wherever_it_is() {
        let heat = movie("Heat", Some(1995));
        let expected = ("Heat".to_string(), Some("1995".to_string()));
        assert_eq!(card_text(&heat, Titled::WithShow), expected);
        assert_eq!(
            card_text(&heat, Titled::InSeason { ringed: None }),
            expected
        );
    }

    #[test]
    fn card_ids_are_told_apart_by_their_row() {
        let heat = movie("Heat", None);
        assert_eq!(item_card_id("movie", &heat).as_ref(), "movie-jf-1-Heat");
        assert_eq!(show_card_id("all", "Dark").as_ref(), "all-show-Dark");
    }
}
