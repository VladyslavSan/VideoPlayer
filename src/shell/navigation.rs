//! Where the viewer is: the screen showing and the tab it belongs to, how
//! its page is scrolled, the page being left while it fades out, the
//! keyboard cursor each screen had when it was left, and the episode a show
//! page was opened at. The shell changes screens here and draws from it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::mem;
use std::ops::Range;
use std::time::{Duration, Instant};

use gpui::{Bounds, Pixels, ScrollHandle, SharedString, UniformListScrollHandle, point, px};

use super::browse::{Origin, Shelf};
use super::header::HEADER_HEIGHT;
use crate::arr::Kind;
use crate::motion::{self, PAGE};

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Screen {
    /// Every source together.
    Home,
    /// One source's page: a Jellyfin server, or None for local folders.
    Source(Origin),
    /// A library's grid of movies, or of shows.
    Movies(Shelf),
    Shows(Shelf),
    Show(String),
    /// A movie's details page, by item key.
    Details(String),
    /// What Sonarr and Radarr are downloading.
    Downloads,
    /// A server's Sonarr or Radarr: adding to it, and what it's doing.
    Arr(i64, Kind),
    /// Something Sonarr or Radarr lists but Jellyfin lacks: server, app, its id.
    Tracked(i64, Kind, i64),
    /// A search result nobody has yet: server, app, its TVDB or TMDB id.
    Found(i64, Kind, i64),
    Settings,
}

impl Screen {
    /// Whether this is a tab of the sidebar, with the search field over it,
    /// not a page opened from one (or Settings).
    pub(super) fn is_tab(&self) -> bool {
        matches!(
            self,
            Screen::Home
                | Screen::Source(_)
                | Screen::Movies(_)
                | Screen::Shows(_)
                | Screen::Downloads
                | Screen::Arr(..)
        )
    }
}

/// Room kept between the card picked with the keyboard and the page's edge.
const CURSOR_MARGIN: f32 = 24.0;
/// How many screens' cursors are remembered; the oldest go first.
const REMEMBERED_CURSORS: usize = 20;
/// How long a show page opened at an episode keeps it in place while the
/// page above it settles.
const EPISODE_SETTLE: Duration = Duration::from_millis(1500);
/// Room above that episode's card for its season's heading.
const ABOVE_EPISODE: f32 = 64.0;

/// A scroll handle at the same place as `handle`, for drawing a page that
/// no longer moves.
fn frozen(handle: &ScrollHandle) -> ScrollHandle {
    let copy = ScrollHandle::new();
    copy.set_offset(handle.offset());
    copy
}

/// A list's scroll handle at the same place as `handle`.
pub(super) fn frozen_list(handle: &UniformListScrollHandle) -> UniformListScrollHandle {
    let copy = UniformListScrollHandle::new();
    copy.0
        .borrow()
        .base_handle
        .set_offset(handle.0.borrow().base_handle.offset());
    copy
}

/// The scroll offset (zero or less, at least `-max`) nearest `offset` that
/// brings `card` inside `view`, both measured along the scrolling axis.
fn offset_showing(offset: Pixels, card: Range<Pixels>, view: Range<Pixels>, max: Pixels) -> Pixels {
    let mut showing = offset;
    if card.start < view.start {
        showing += view.start - card.start;
    } else if card.end > view.end {
        showing -= card.end - view.end;
    }
    showing.clamp(-max, px(0.0))
}

/// The scroll offset that puts a card, drawn at `card_top` while the page
/// is at `offset`, just under the header and its season's heading.
fn offset_for_episode(
    card_top: Pixels,
    offset: Pixels,
    viewport_top: Pixels,
    max: Pixels,
) -> Pixels {
    let in_content = card_top - offset;
    (viewport_top + px(HEADER_HEIGHT + ABOVE_EPISODE) - in_content).clamp(-max, px(0.0))
}

/// The page's scrolling: the page itself, a library grid, the home rows.
struct Scrolls {
    /// The scrolling page (everything but the virtualized grids).
    content: ScrollHandle,
    grid: UniformListScrollHandle,
    /// Sideways, by row id; made as the rows are first drawn.
    rows: RefCell<HashMap<SharedString, ScrollHandle>>,
}

impl Scrolls {
    fn new() -> Self {
        Self {
            content: ScrollHandle::new(),
            grid: UniformListScrollHandle::new(),
            rows: RefCell::default(),
        }
    }

    /// Scrolls the page, and the row the card is in, so `card` is in view.
    fn reveal(&self, card: Bounds<Pixels>) {
        let margin = px(CURSOR_MARGIN);
        let page = &self.content;
        let (view, offset, max) = (page.bounds(), page.offset(), page.max_offset());
        let y = offset_showing(
            offset.y,
            card.top()..card.bottom(),
            view.top() + px(HEADER_HEIGHT) + margin..view.bottom() - margin,
            max.height,
        );
        if y != offset.y {
            page.set_offset(point(offset.x, y));
        }
        let center = card.top() + card.size.height / 2.0;
        for row in self.rows.borrow().values() {
            let view = row.bounds();
            if center < view.top() || center > view.bottom() {
                continue;
            }
            let (offset, max) = (row.offset(), row.max_offset());
            let x = offset_showing(
                offset.x,
                card.left()..card.right(),
                view.left()..view.right(),
                max.width,
            );
            if x != offset.x {
                row.set_offset(point(x, offset.y));
            }
        }
    }
}

/// The page being left, fading out under the new one: drawn as it was,
/// scrolled where it was, until the new page has faded in.
struct Leaving {
    screen: Screen,
    started: Instant,
    content_scroll: ScrollHandle,
    grid_scroll: UniformListScrollHandle,
}

/// A screen's cursor when it was left.
struct Remembered {
    screen: Screen,
    cursor: SharedString,
}

/// Each screen's cursor when it was left, to put back on return.
#[derive(Default)]
struct Cursors(Vec<Remembered>);

impl Cursors {
    fn remember(&mut self, screen: Screen, cursor: SharedString) {
        self.0.retain(|remembered| remembered.screen != screen);
        self.0.push(Remembered { screen, cursor });
        if self.0.len() > REMEMBERED_CURSORS {
            self.0.remove(0);
        }
    }

    fn recall(&self, screen: &Screen) -> Option<SharedString> {
        self.0
            .iter()
            .find(|remembered| remembered.screen == *screen)
            .map(|remembered| remembered.cursor.clone())
    }
}

/// Keeping a show page at the episode it was opened at, while it settles.
struct EpisodeScroll {
    until: Instant,
    /// The offset last set; a different one means the viewer scrolled.
    applied: Option<Pixels>,
}

pub(super) struct Navigation {
    screen: Screen,
    /// The library tab to go back to from a show page.
    tab: Screen,
    /// The screen the page was last drawn for; a new one starts at the top.
    drawn: Option<Screen>,
    /// The screen whose cards were last noted: a frame behind a new screen.
    cards_screen: Option<Screen>,
    scrolls: Scrolls,
    leaving: Option<Leaving>,
    cursors: Cursors,
    /// Bring the cursor's card into view once it has been drawn.
    reveal_cursor: bool,
    /// On a show page: the episode it was opened at (by item key), ringed.
    episode: Option<String>,
    episode_scroll: Option<EpisodeScroll>,
}

impl Navigation {
    pub(super) fn new() -> Self {
        Self {
            screen: Screen::Home,
            tab: Screen::Home,
            drawn: None,
            cards_screen: None,
            scrolls: Scrolls::new(),
            leaving: None,
            cursors: Cursors::default(),
            reveal_cursor: false,
            episode: None,
            episode_scroll: None,
        }
    }

    pub(super) fn screen(&self) -> &Screen {
        &self.screen
    }

    pub(super) fn tab(&self) -> &Screen {
        &self.tab
    }

    /// Shows `screen`, a page within the current tab.
    pub(super) fn open(&mut self, screen: Screen) {
        self.screen = screen;
    }

    /// Shows `screen` as the tab to come back to.
    pub(super) fn open_tab(&mut self, screen: Screen) {
        self.tab = screen.clone();
        self.screen = screen;
    }

    /// Goes back from a page to the tab it was opened from.
    pub(super) fn back_to_tab(&mut self) {
        self.screen = self.tab.clone();
    }

    pub(super) fn content_scroll(&self) -> &ScrollHandle {
        &self.scrolls.content
    }

    pub(super) fn grid_scroll(&self) -> &UniformListScrollHandle {
        &self.scrolls.grid
    }

    /// The sideways scrolling of home row `id`.
    pub(super) fn row_scroll(&self, id: &SharedString) -> ScrollHandle {
        let mut rows = self.scrolls.rows.borrow_mut();
        rows.entry(id.clone()).or_default().clone()
    }

    /// Scrolls the page, and the row the card is in, so `card` is in view.
    pub(super) fn reveal(&self, card: Bounds<Pixels>) {
        self.scrolls.reveal(card);
    }

    /// Whether the screen changed since its page was last drawn.
    pub(super) fn is_new_screen(&self) -> bool {
        self.drawn.as_ref() != Some(&self.screen)
    }

    /// The new screen's page is about to be drawn, from the top. `cursor`
    /// was the last screen's, kept for it; returns the one this screen had.
    pub(super) fn screen_drawn(&mut self, cursor: Option<SharedString>) -> Option<SharedString> {
        if let (Some(old), Some(cursor)) = (self.drawn.take(), cursor) {
            self.cursors.remember(old, cursor);
        }
        let cursor = self.cursors.recall(&self.screen);
        self.reveal_cursor = cursor.is_some();
        self.drawn = Some(self.screen.clone());
        self.scrolls.content.set_offset(point(px(0.0), px(0.0)));
        cursor
    }

    /// Whether the cursor put back on return still has to be scrolled to.
    pub(super) fn reveals_cursor(&self) -> bool {
        self.reveal_cursor
    }

    pub(super) fn cursor_revealed(&mut self) {
        self.reveal_cursor = false;
    }

    /// This frame's cards are the current screen's.
    pub(super) fn cards_noted(&mut self) {
        self.cards_screen = Some(self.screen.clone());
    }

    /// Whether the cards last noted were drawn for the current screen.
    pub(super) fn cards_are_current(&self) -> bool {
        self.cards_screen.as_ref() == Some(&self.screen)
    }

    /// The page last drawn starts fading out, scrolled where it was. False
    /// if it doesn't fade: motion is off, or Settings is either side.
    pub(super) fn start_leaving(&mut self) -> bool {
        let Some(old) = self.drawn.clone() else {
            return false;
        };
        if !motion::enabled() || old == Screen::Settings || self.screen == Screen::Settings {
            self.leaving = None;
            return false;
        }
        self.leaving = Some(Leaving {
            screen: old,
            started: Instant::now(),
            content_scroll: frozen(&self.scrolls.content),
            grid_scroll: frozen_list(&self.scrolls.grid),
        });
        true
    }

    /// The page being left is gone: the new one has faded in.
    pub(super) fn stop_leaving(&mut self) {
        self.leaving = None;
    }

    /// The screen being left, while its page is still fading out; once the
    /// fade is over, the page is gone.
    pub(super) fn fading_out(&mut self) -> Option<Screen> {
        let leaving = self.leaving.as_ref()?;
        if leaving.started.elapsed() >= PAGE.time() {
            self.leaving = None;
            return None;
        }
        Some(leaving.screen.clone())
    }

    /// Swaps the page's scroll positions with the leaving page's, to draw
    /// that one; and back, called again.
    pub(super) fn swap_leaving_scrolls(&mut self) {
        let Some(leaving) = self.leaving.as_mut() else {
            return;
        };
        mem::swap(&mut self.scrolls.content, &mut leaving.content_scroll);
        mem::swap(&mut self.scrolls.grid, &mut leaving.grid_scroll);
    }

    /// The episode the show page was opened at, by item key.
    pub(super) fn episode(&self) -> Option<&str> {
        self.episode.as_deref()
    }

    /// The show page opens at this episode: ringed, and scrolled to.
    pub(super) fn open_at_episode(&mut self, key: String) {
        self.episode = Some(key);
        self.episode_scroll = Some(EpisodeScroll {
            until: Instant::now() + EPISODE_SETTLE,
            applied: None,
        });
    }

    pub(super) fn forget_episode(&mut self) {
        self.episode = None;
    }

    /// Scrolls the show page to its episode's `card` (None: not drawn yet).
    /// True while the page may still grow above it: call again next frame.
    pub(super) fn follow_episode(&mut self, card: Option<Bounds<Pixels>>) -> bool {
        let Some(following) = &mut self.episode_scroll else {
            return false;
        };
        let page = &self.scrolls.content;
        let offset = page.offset();
        let scrolled_away = following.applied.is_some_and(|y| y != offset.y);
        if Instant::now() >= following.until || scrolled_away {
            self.episode_scroll = None;
            return false;
        }
        let Some(card) = card else {
            return true;
        };
        let (viewport, max) = (page.bounds(), page.max_offset());
        let y = offset_for_episode(card.top(), offset.y, viewport.top(), max.height);
        if y != offset.y {
            page.set_offset(point(offset.x, y));
        }
        following.applied = Some(y);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn show(name: &str) -> Screen {
        Screen::Show(name.to_string())
    }

    #[test]
    fn tabs_are_the_screens_the_sidebar_leads_to() {
        let local = Shelf {
            origin: None,
            library: None,
        };
        assert!(Screen::Home.is_tab());
        assert!(Screen::Source(Some(1)).is_tab());
        assert!(Screen::Movies(local).is_tab());
        assert!(Screen::Downloads.is_tab());
        assert!(Screen::Arr(1, Kind::Sonarr).is_tab());
        assert!(!show("Dark").is_tab());
        assert!(!Screen::Details("jf-1-a".into()).is_tab());
        assert!(!Screen::Tracked(1, Kind::Radarr, 7).is_tab());
        assert!(!Screen::Settings.is_tab());
    }

    #[test]
    fn a_screen_gets_its_cursor_back_on_return() {
        let mut cursors = Cursors::default();
        cursors.remember(Screen::Home, "movie-1".into());
        cursors.remember(show("Dark"), "episode-4".into());
        assert_eq!(cursors.recall(&Screen::Home), Some("movie-1".into()));
        assert_eq!(cursors.recall(&show("Dark")), Some("episode-4".into()));
        assert_eq!(cursors.recall(&Screen::Downloads), None);
    }

    #[test]
    fn leaving_a_screen_again_replaces_its_cursor() {
        let mut cursors = Cursors::default();
        cursors.remember(Screen::Home, "movie-1".into());
        cursors.remember(Screen::Home, "movie-2".into());
        assert_eq!(cursors.recall(&Screen::Home), Some("movie-2".into()));
        assert_eq!(cursors.0.len(), 1);
    }

    #[test]
    fn only_the_last_twenty_screens_keep_their_cursor() {
        let mut cursors = Cursors::default();
        for n in 0..=REMEMBERED_CURSORS {
            cursors.remember(show(&format!("show {n}")), "card".into());
        }
        assert_eq!(cursors.0.len(), REMEMBERED_CURSORS);
        assert_eq!(cursors.recall(&show("show 0")), None);
        assert_eq!(cursors.recall(&show("show 1")), Some("card".into()));
        assert_eq!(cursors.recall(&show("show 20")), Some("card".into()));
    }

    #[test]
    fn a_new_screen_takes_the_old_ones_cursor_and_gives_its_own() {
        let mut navigation = Navigation::new();
        assert!(navigation.is_new_screen());
        assert_eq!(navigation.screen_drawn(None), None);
        assert!(!navigation.is_new_screen());

        navigation.open(show("Dark"));
        assert!(navigation.is_new_screen());
        assert_eq!(navigation.screen_drawn(Some("home-card".into())), None);
        assert!(!navigation.reveals_cursor());

        navigation.back_to_tab();
        let cursor = navigation.screen_drawn(Some("episode-4".into()));
        assert_eq!(cursor, Some("home-card".into()));
        assert!(navigation.reveals_cursor());
    }

    #[test]
    fn a_page_goes_back_to_the_tab_it_was_opened_from() {
        let mut navigation = Navigation::new();
        navigation.open_tab(Screen::Downloads);
        navigation.open(show("Dark"));
        assert_eq!(*navigation.tab(), Screen::Downloads);
        navigation.back_to_tab();
        assert_eq!(*navigation.screen(), Screen::Downloads);
    }

    #[test]
    fn cards_belong_to_a_new_screen_only_once_noted() {
        let mut navigation = Navigation::new();
        navigation.cards_noted();
        assert!(navigation.cards_are_current());
        navigation.open(show("Dark"));
        assert!(!navigation.cards_are_current());
        navigation.cards_noted();
        assert!(navigation.cards_are_current());
    }

    #[test]
    fn nothing_fades_out_before_the_first_page() {
        let mut navigation = Navigation::new();
        assert!(!navigation.start_leaving());
        assert_eq!(navigation.fading_out(), None);
    }

    #[test]
    fn a_card_in_view_leaves_the_scroll_alone() {
        let offset = offset_showing(
            px(-100.0),
            px(200.0)..px(400.0),
            px(0.0)..px(800.0),
            px(900.0),
        );
        assert_eq!(offset, px(-100.0));
    }

    #[test]
    fn a_card_above_the_view_scrolls_down_to_its_top() {
        let offset = offset_showing(
            px(-100.0),
            px(-50.0)..px(150.0),
            px(0.0)..px(800.0),
            px(900.0),
        );
        assert_eq!(offset, px(-50.0));
    }

    #[test]
    fn a_card_below_the_view_scrolls_up_to_its_bottom() {
        let offset = offset_showing(
            px(-100.0),
            px(700.0)..px(900.0),
            px(0.0)..px(800.0),
            px(900.0),
        );
        assert_eq!(offset, px(-200.0));
    }

    #[test]
    fn scrolling_to_a_card_stops_at_the_ends() {
        let top = offset_showing(
            px(-20.0),
            px(-50.0)..px(150.0),
            px(0.0)..px(800.0),
            px(900.0),
        );
        assert_eq!(top, px(0.0));
        let bottom = offset_showing(
            px(-850.0),
            px(700.0)..px(900.0),
            px(0.0)..px(800.0),
            px(900.0),
        );
        assert_eq!(bottom, px(-900.0));
    }

    #[test]
    fn an_episode_is_put_under_the_header_and_its_seasons_heading() {
        // Drawn 1000 down an unscrolled page: scrolled so it sits 144 down.
        let offset = offset_for_episode(px(1000.0), px(0.0), px(0.0), px(5000.0));
        assert_eq!(offset, px(HEADER_HEIGHT + ABOVE_EPISODE - 1000.0));
        // Already there: the same offset again.
        let card_top = px(1000.0) + offset;
        assert_eq!(
            offset_for_episode(card_top, offset, px(0.0), px(5000.0)),
            offset
        );
        // A short page can't scroll that far.
        assert_eq!(
            offset_for_episode(px(1000.0), px(0.0), px(0.0), px(300.0)),
            px(-300.0)
        );
    }

    #[test]
    fn a_show_page_follows_its_episode_until_the_viewer_scrolls() {
        let mut navigation = Navigation::new();
        assert!(!navigation.follow_episode(None));
        navigation.open_at_episode("episode-4".to_string());
        assert_eq!(navigation.episode(), Some("episode-4"));
        // Not drawn yet: asked again next frame.
        assert!(navigation.follow_episode(None));
        let card = Bounds::new(point(px(0.0), px(40.0)), gpui::size(px(256.0), px(144.0)));
        assert!(navigation.follow_episode(Some(card)));
        // The viewer scrolled: the page is theirs again.
        navigation
            .content_scroll()
            .set_offset(point(px(0.0), px(-30.0)));
        assert!(!navigation.follow_episode(Some(card)));
        assert!(!navigation.follow_episode(Some(card)));
    }
}
