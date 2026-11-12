//! Moving between cards with the arrow keys: on pages, to the nearest card
//! in that direction as drawn; in the virtualized grids, by index.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

impl Direction {
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            _ => None,
        }
    }
}

/// A card's box on screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Rect {
    fn center_x(&self) -> f32 {
        (self.left + self.right) / 2.0
    }

    fn center_y(&self) -> f32 {
        (self.top + self.bottom) / 2.0
    }

    fn overlaps_vertically(&self, other: &Rect) -> bool {
        self.top < other.bottom && other.top < self.bottom
    }
}

/// The card to move to from `from`: sideways, the closest one in the same
/// row; up or down, the closest row, then the card most in line with
/// `from` there. None at the edge.
pub fn nearest(from: Rect, cards: &[Rect], direction: Direction) -> Option<usize> {
    let candidates = cards.iter().enumerate().filter(|(_, c)| match direction {
        Direction::Left => c.center_x() < from.center_x() - 1.0 && c.overlaps_vertically(&from),
        Direction::Right => c.center_x() > from.center_x() + 1.0 && c.overlaps_vertically(&from),
        Direction::Up => c.center_y() < from.center_y() - 1.0 && !c.overlaps_vertically(&from),
        Direction::Down => c.center_y() > from.center_y() + 1.0 && !c.overlaps_vertically(&from),
    });
    let key = |c: &Rect| -> (f32, f32) {
        match direction {
            Direction::Left => (from.left - c.right, 0.0),
            Direction::Right => (c.left - from.right, 0.0),
            // Rows first (rounded, so cards of one row tie), then alignment.
            Direction::Up => (
                (from.top - c.bottom).round(),
                (c.center_x() - from.center_x()).abs(),
            ),
            Direction::Down => (
                (c.top - from.bottom).round(),
                (c.center_x() - from.center_x()).abs(),
            ),
        }
    };
    candidates
        .min_by(|(_, a), (_, b)| {
            key(a)
                .partial_cmp(&key(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
}

/// Moves `index` within a grid of `count` cards, `columns` wide. Down from
/// the last full row lands on the last card. None at the edge.
pub fn grid_step(
    index: usize,
    count: usize,
    columns: usize,
    direction: Direction,
) -> Option<usize> {
    let columns = columns.max(1);
    let next = match direction {
        Direction::Left => (!index.is_multiple_of(columns)).then(|| index - 1)?,
        Direction::Right => {
            (index % columns + 1 < columns && index + 1 < count).then(|| index + 1)?
        }
        Direction::Up => index.checked_sub(columns)?,
        Direction::Down => {
            let below = index + columns;
            let last_row = (count - 1) / columns;
            if index / columns >= last_row {
                return None;
            }
            below.min(count - 1)
        }
    };
    (next < count).then_some(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(col: f32, row: f32) -> Rect {
        Rect {
            left: col * 340.0,
            top: row * 260.0,
            right: col * 340.0 + 320.0,
            bottom: row * 260.0 + 240.0,
        }
    }

    #[test]
    fn sideways_stays_in_the_row() {
        let cards = [
            card(0.0, 0.0),
            card(1.0, 0.0),
            card(2.0, 0.0),
            card(0.0, 1.0),
        ];
        assert_eq!(nearest(cards[0], &cards, Direction::Right), Some(1));
        assert_eq!(nearest(cards[1], &cards, Direction::Left), Some(0));
        assert_eq!(nearest(cards[2], &cards, Direction::Right), None);
        assert_eq!(nearest(cards[3], &cards, Direction::Right), None);
    }

    #[test]
    fn up_and_down_pick_the_nearest_row_then_the_aligned_card() {
        // A short row above a longer one; the third row is further down.
        let cards = [
            card(0.0, 0.0),
            card(0.0, 1.0),
            card(1.0, 1.0),
            card(2.0, 1.0),
            card(2.0, 2.5),
        ];
        assert_eq!(nearest(cards[0], &cards, Direction::Down), Some(1));
        assert_eq!(nearest(cards[3], &cards, Direction::Up), Some(0));
        assert_eq!(nearest(cards[2], &cards, Direction::Down), Some(4));
        assert_eq!(nearest(cards[0], &cards, Direction::Up), None);
    }

    #[test]
    fn rows_scrolled_sideways_still_line_up() {
        // The upper row is scrolled: its cards sit off a 340 px grid.
        let shifted = |col: f32| Rect {
            left: col * 340.0 - 150.0,
            top: 0.0,
            right: col * 340.0 + 170.0,
            bottom: 240.0,
        };
        let cards = [shifted(0.0), shifted(1.0), shifted(2.0), card(1.0, 1.0)];
        assert_eq!(nearest(cards[3], &cards, Direction::Up), Some(1));
    }

    #[test]
    fn grid_steps() {
        // 7 cards, 3 across: rows 0-2, 3-5, 6.
        assert_eq!(grid_step(0, 7, 3, Direction::Right), Some(1));
        assert_eq!(grid_step(2, 7, 3, Direction::Right), None);
        assert_eq!(grid_step(3, 7, 3, Direction::Left), None);
        assert_eq!(grid_step(4, 7, 3, Direction::Up), Some(1));
        assert_eq!(grid_step(1, 7, 3, Direction::Up), None);
        assert_eq!(grid_step(1, 7, 3, Direction::Down), Some(4));
        assert_eq!(grid_step(5, 7, 3, Direction::Down), Some(6));
        assert_eq!(grid_step(6, 7, 3, Direction::Down), None);
        assert_eq!(grid_step(6, 7, 3, Direction::Right), None);
    }
}
