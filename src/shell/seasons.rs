//! The seasons on show pages: which are open (the rest are folded), and the
//! folding itself, a season's episodes growing from nothing or shrinking to
//! it over their real height, measured each time they're drawn.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use std::time::Instant;

use gpui::{Canvas, Styled, canvas};

use super::cards;
use crate::motion::{self, FOLD};

/// A season: its show's name and its number (0: specials).
type Key = (String, u16);

/// Padding around a season's episodes, inside the part that folds.
const PADDING: f32 = 24.0;
/// A card's text and the gap to the next row of cards.
const BELOW_CARD: f32 = 56.0;
/// A listed episode's line (one Jellyfin doesn't have).
const LINE_HEIGHT: f32 = 76.0;

/// What a season shows when open, for guessing its height before it's drawn.
#[derive(Debug, Clone, Copy)]
pub(super) enum Episodes {
    /// This many cards, in a grid.
    Cards(usize),
    /// This many lines, one under another.
    Lines(usize),
}

/// A season folding open or shut.
struct Fold {
    opening: bool,
    started: Instant,
    /// Counts folds, to key this one's animation.
    key: u64,
}

/// A fold to draw: which way it goes, and the key of its animation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Folding {
    pub opening: bool,
    pub key: u64,
    /// Shutting and not yet shut: the episodes are still drawn.
    pub closing: bool,
}

pub(super) struct Seasons {
    /// The seasons open on each show's page.
    open: HashMap<String, BTreeSet<u16>>,
    folds: HashMap<Key, Fold>,
    fold_count: u64,
    /// Each open season's height as last drawn.
    heights: Rc<RefCell<HashMap<Key, f32>>>,
    /// Cards across, as last laid out (for a season's height while it folds).
    columns: usize,
}

impl Seasons {
    pub(super) fn new() -> Self {
        Self {
            open: HashMap::new(),
            folds: HashMap::new(),
            fold_count: 0,
            heights: Rc::default(),
            columns: 4,
        }
    }

    pub(super) fn is_open(&self, show: &str, number: u16) -> bool {
        self.open
            .get(show)
            .is_some_and(|open| open.contains(&number))
    }

    /// Opens just these seasons of `show`, as its page opens.
    pub(super) fn open_only(&mut self, show: &str, numbers: impl IntoIterator<Item = u16>) {
        self.open
            .insert(show.to_string(), numbers.into_iter().collect());
    }

    /// Opens another season of `show`, without folding it open.
    pub(super) fn open_too(&mut self, show: &str, number: u16) {
        self.open
            .entry(show.to_string())
            .or_default()
            .insert(number);
    }

    /// Opens a closed season or closes an open one, folding it.
    pub(super) fn toggle(&mut self, show: &str, number: u16) {
        let open = self.open.entry(show.to_string()).or_default();
        let opening = !open.remove(&number);
        if opening {
            open.insert(number);
        }
        self.fold_count += 1;
        self.folds.insert(
            (show.to_string(), number),
            Fold {
                opening,
                started: Instant::now(),
                key: self.fold_count,
            },
        );
    }

    /// The season's last fold, to draw it with: none without motion.
    pub(super) fn folding(&self, show: &str, number: u16) -> Option<Folding> {
        let fold = self.folds.get(&(show.to_string(), number))?;
        motion::enabled().then(|| Folding {
            opening: fold.opening,
            key: fold.key,
            closing: !fold.opening && fold.started.elapsed() < FOLD.time(),
        })
    }

    /// The cards across the page, as just laid out.
    pub(super) fn set_columns(&mut self, columns: usize) {
        self.columns = columns;
    }

    /// The height the season's episodes fold over: as last drawn, or a
    /// guess from `episodes`, corrected after the first frame.
    pub(super) fn height(&self, show: &str, number: u16, episodes: Episodes) -> f32 {
        let measured = self
            .heights
            .borrow()
            .get(&(show.to_string(), number))
            .copied();
        measured.unwrap_or_else(|| guessed_height(episodes, self.columns))
    }

    /// An element that fills the season's episodes and notes their height.
    pub(super) fn height_marker(&self, show: &str, number: u16) -> Canvas<()> {
        let heights = self.heights.clone();
        let key = (show.to_string(), number);
        canvas(
            move |bounds, _, _| {
                heights
                    .borrow_mut()
                    .insert(key, f32::from(bounds.size.height));
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }
}

/// How tall a season's episodes are likely to be, `columns` cards across.
fn guessed_height(episodes: Episodes, columns: usize) -> f32 {
    match episodes {
        Episodes::Lines(count) => PADDING + count as f32 * LINE_HEIGHT,
        Episodes::Cards(count) => {
            let rows = count.div_ceil(columns.max(1));
            PADDING + rows as f32 * (cards::HEIGHT + BELOW_CARD)
        }
    }
}

/// How much of a folding season shows, `progress` (0 to 1) into its fold.
pub(super) fn shown(opening: bool, progress: f32) -> f32 {
    if opening { progress } else { 1.0 - progress }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_show_page_opens_with_only_the_season_being_watched() {
        let mut seasons = Seasons::new();
        seasons.open_only("Dark", [2]);
        assert!(seasons.is_open("Dark", 2));
        assert!(!seasons.is_open("Dark", 1));
        assert!(!seasons.is_open("Another", 2));
        // Opening the page again starts over.
        seasons.open_too("Dark", 1);
        seasons.open_only("Dark", None);
        assert!(!seasons.is_open("Dark", 1));
        assert!(!seasons.is_open("Dark", 2));
    }

    #[test]
    fn a_season_opened_with_its_page_does_not_fold() {
        let mut seasons = Seasons::new();
        seasons.open_too("Dark", 3);
        assert!(seasons.is_open("Dark", 3));
        assert_eq!(seasons.folding("Dark", 3), None);
    }

    #[test]
    fn toggling_a_season_opens_it_then_closes_it() {
        let mut seasons = Seasons::new();
        seasons.toggle("Dark", 1);
        assert!(seasons.is_open("Dark", 1));
        seasons.toggle("Dark", 1);
        assert!(!seasons.is_open("Dark", 1));
        assert_eq!(seasons.fold_count, 2);
    }

    #[test]
    fn each_fold_gets_its_own_key() {
        if !motion::enabled() {
            return;
        }
        let mut seasons = Seasons::new();
        seasons.toggle("Dark", 1);
        let opening = seasons.folding("Dark", 1).expect("just toggled");
        assert!(opening.opening && !opening.closing);
        seasons.toggle("Dark", 1);
        let closing = seasons.folding("Dark", 1).expect("just toggled");
        assert!(!closing.opening && closing.closing);
        assert_ne!(opening.key, closing.key);
    }

    #[test]
    fn a_season_not_drawn_yet_gets_a_height_from_its_episodes() {
        let seasons = Seasons::new();
        // Nine cards, four across: three rows.
        let cards = seasons.height("Dark", 1, Episodes::Cards(9));
        assert_eq!(cards, PADDING + 3.0 * (cards::HEIGHT + BELOW_CARD));
        let lines = seasons.height("Dark", 1, Episodes::Lines(5));
        assert_eq!(lines, PADDING + 5.0 * LINE_HEIGHT);
    }

    #[test]
    fn a_season_drawn_before_folds_over_its_measured_height() {
        let seasons = Seasons::new();
        seasons
            .heights
            .borrow_mut()
            .insert(("Dark".to_string(), 1), 512.0);
        assert_eq!(seasons.height("Dark", 1, Episodes::Cards(9)), 512.0);
    }

    #[test]
    fn a_season_with_no_columns_still_has_a_height() {
        assert_eq!(
            guessed_height(Episodes::Cards(2), 0),
            PADDING + 2.0 * (cards::HEIGHT + BELOW_CARD)
        );
    }

    #[test]
    fn an_opening_fold_shows_more_and_a_closing_one_less() {
        assert_eq!(shown(true, 0.25), 0.25);
        assert_eq!(shown(false, 0.25), 0.75);
        assert_eq!(shown(false, 1.0), 0.0);
    }
}
