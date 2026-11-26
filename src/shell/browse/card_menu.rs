//! Marking things watched or unwatched by hand, from a right-click menu: a
//! movie or episode on its card, a season on its header, a show on its card.
//! The cards change at once; the server is told in the background.

use gpui::{AnyElement, ClickEvent, Context, Pixels, Point, Window, prelude::*};

use super::Pages;
use crate::library::Item;
use crate::ui::menu;

/// What a card's menu acts on.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum MenuTarget {
    /// A movie or episode, by library key.
    Item(String),
    /// A show, by name.
    Show(String),
    /// A season of a show (0: specials).
    Season { show: String, number: u16 },
}

/// A right-click menu, where the click was. Its choices are made when it
/// opens, so they don't change as it fades out after one is picked.
pub(super) struct CardMenu {
    at: Point<Pixels>,
    choices: Vec<Choice>,
}

/// A choice in a card's menu.
#[derive(Clone, Debug, PartialEq)]
struct Choice {
    label: String,
    /// The items it marks, by key.
    keys: Vec<String>,
    /// Marks them watched; else unwatched.
    watched: bool,
}

/// `1 episode`, `3 episodes`: says how many, so a whole season isn't
/// marked by surprise.
fn episode_count(count: usize) -> String {
    if count == 1 {
        "1 episode".to_string()
    } else {
        format!("{count} episodes")
    }
}

/// The choices for one movie or episode: only this one is marked.
fn item_choices(item: &Item) -> Vec<Choice> {
    let unwatch = item.watched && item.progress().is_none();
    let label = if unwatch {
        "Mark as unwatched"
    } else {
        "Mark as watched"
    };
    vec![Choice {
        label: label.to_string(),
        keys: vec![item.key()],
        watched: !unwatch,
    }]
}

/// The choices for a show's episodes, or those of one `season` of it.
fn episode_choices(episodes: &[&Item], season: Option<u16>) -> Vec<Choice> {
    let what = match season {
        None => "show".to_string(),
        Some(0) => "specials".to_string(),
        Some(n) => format!("season {n}"),
    };
    let keys: Vec<String> = episodes.iter().map(|i| i.key()).collect();
    let mut choices = Vec::new();
    let to_watch = episodes.iter().filter(|i| !i.watched).count();
    if to_watch > 0 {
        choices.push(Choice {
            label: format!("Mark {what} as watched ({})", episode_count(to_watch)),
            keys: keys.clone(),
            watched: true,
        });
    }
    let to_unwatch = episodes
        .iter()
        .filter(|i| i.watched || i.progress().is_some())
        .count();
    if to_unwatch > 0 {
        choices.push(Choice {
            label: format!("Mark {what} as unwatched ({})", episode_count(to_unwatch)),
            keys,
            watched: false,
        });
    }
    choices
}

/// The choices for a card's menu, given the library's `items`.
fn choices<'a>(mut items: impl Iterator<Item = &'a Item>, target: &MenuTarget) -> Vec<Choice> {
    let (name, season) = match target {
        MenuTarget::Item(key) => {
            let item = items.find(|i| i.key() == *key);
            return item.map(item_choices).unwrap_or_default();
        }
        MenuTarget::Show(name) => (name, None),
        MenuTarget::Season { show, number } => (show, Some(*number)),
    };
    let episodes: Vec<&Item> = items
        .filter(|i| i.show.as_deref() == Some(name.as_str()))
        .filter(|i| season.is_none_or(|s| i.season.unwrap_or(0) == s))
        .collect();
    episode_choices(&episodes, season)
}

impl Pages {
    pub(super) fn open_card_menu(
        &mut self,
        target: MenuTarget,
        at: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let choices = choices(self.items(), &target);
        if choices.is_empty() {
            self.close_card_menu(cx);
            return;
        }
        self.card_menu.open(CardMenu { at, choices });
        cx.notify();
    }

    /// Closes the menu; false if none was open.
    pub(in crate::shell) fn close_card_menu(&mut self, cx: &mut Context<Self>) -> bool {
        let open = self.card_menu.close();
        if open {
            cx.notify();
        }
        open
    }

    /// A choice was picked: the menu closes and its items are marked.
    fn choose(&mut self, choice: &Choice, cx: &mut Context<Self>) {
        self.card_menu.close();
        self.mark_watched(&choice.keys, choice.watched, cx);
    }

    /// The right-click menu, and any still fading out.
    pub(in crate::shell) fn card_menu_view(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        self.card_menu
            .drawn()
            .iter()
            .map(|shown| {
                let choices = shown.value.choices.iter().cloned().enumerate();
                let items = choices
                    .map(|(i, choice)| {
                        menu::item(("card-menu", i), choice.label.clone(), false)
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.choose(&choice, cx)
                            }))
                            .into_any_element()
                    })
                    .collect();
                let pages = cx.entity().downgrade();
                menu::context(shown, shown.value.at, items, window, move |_, _, cx| {
                    pages.update(cx, |this, cx| this.close_card_menu(cx)).ok();
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Source;

    fn episode(season: u16, number: u16, watched: bool) -> Item {
        Item {
            library: None,
            source: Source::Jellyfin {
                server: 1,
                id: format!("s{season}e{number}"),
            },
            title: format!("Episode {number}"),
            year: None,
            show: Some("Dark".into()),
            season: Some(season),
            episode: Some(number),
            duration: Some(3000.0),
            position: 0.0,
            watched,
            last_played: None,
            thumb: None,
            added: None,
        }
    }

    fn labels(choices: &[Choice]) -> Vec<&str> {
        choices.iter().map(|c| c.label.as_str()).collect()
    }

    #[test]
    fn an_unwatched_item_can_be_marked_watched_and_back() {
        let unwatched = episode(1, 1, false);
        let target = MenuTarget::Item(unwatched.key());
        let menu = choices([&unwatched].into_iter(), &target);
        assert_eq!(labels(&menu), ["Mark as watched"]);
        assert!(menu[0].watched);
        assert_eq!(menu[0].keys, [unwatched.key()]);
        let watched = episode(1, 1, true);
        let menu = choices([&watched].into_iter(), &target);
        assert_eq!(labels(&menu), ["Mark as unwatched"]);
        assert!(!menu[0].watched);
    }

    #[test]
    fn a_watched_item_begun_again_is_marked_watched_first() {
        let mut again = episode(1, 1, true);
        again.position = 600.0;
        let target = MenuTarget::Item(again.key());
        let menu = choices([&again].into_iter(), &target);
        assert_eq!(labels(&menu), ["Mark as watched"]);
    }

    #[test]
    fn an_item_no_longer_in_the_library_has_no_menu() {
        let target = MenuTarget::Item("jf-1-gone".into());
        assert!(choices([&episode(1, 1, false)].into_iter(), &target).is_empty());
    }

    #[test]
    fn a_season_says_how_many_episodes_each_choice_marks() {
        let items = [
            episode(1, 1, true),
            episode(1, 2, false),
            episode(1, 3, false),
            episode(2, 1, false),
        ];
        let target = MenuTarget::Season {
            show: "Dark".into(),
            number: 1,
        };
        let menu = choices(items.iter(), &target);
        let expected = [
            "Mark season 1 as watched (2 episodes)",
            "Mark season 1 as unwatched (1 episode)",
        ];
        assert_eq!(labels(&menu), expected);
        // Either choice marks the whole season.
        assert_eq!(menu[0].keys.len(), 3);
        assert_eq!(menu[1].keys.len(), 3);
    }

    #[test]
    fn a_show_offers_only_what_would_change_something() {
        let items = [episode(0, 1, false), episode(1, 1, false)];
        let menu = choices(items.iter(), &MenuTarget::Show("Dark".into()));
        assert_eq!(labels(&menu), ["Mark show as watched (2 episodes)"]);
        let specials = MenuTarget::Season {
            show: "Dark".into(),
            number: 0,
        };
        let menu = choices(items.iter(), &specials);
        assert_eq!(labels(&menu), ["Mark specials as watched (1 episode)"]);
    }
}
