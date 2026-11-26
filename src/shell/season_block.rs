//! One season on a show's page: a row that opens and closes it (with what
//! Sonarr says about it), and its episodes when open, folding. Whichever
//! view shows the page draws it, with its own open seasons.

use gpui::{
    Animation, AnimationExt as _, AnyElement, ClickEvent, Context, FontWeight, MouseButton,
    MouseDownEvent, Pixels, Point, SharedString, div, prelude::*, px, rgb, rgba,
};

use super::episode_list::{self, EpisodeLine};
use super::page_motion::Motion;
use super::seasons::{self, Seasons};
use super::{MUTED, widgets};

/// Makes a season's cards, for the view `V` that draws them.
type DrawCards<'a, V> = Box<dyn FnOnce(&mut Context<V>) -> Vec<AnyElement> + 'a>;
/// What a right click on a season's row does, given where it was.
type RowMenu<V> = Box<dyn Fn(&mut V, Point<Pixels>, &mut Context<V>)>;

/// What the view drawing the page lends to each of its seasons.
pub(super) struct Host<'a, V> {
    pub seasons: &'a Seasons,
    pub motion: &'a Motion,
    /// Opens or closes a season (of a show, by number) in the view.
    pub toggle: fn(&mut V, &str, u16),
}

/// A season to draw.
pub(super) struct Season<'a, V> {
    pub show: &'a str,
    pub number: u16,
    pub episodes: Episodes<'a, V>,
    pub sonarr: Option<SonarrSay>,
}

/// A season's episodes.
pub(super) enum Episodes<'a, V> {
    /// The library has them: cards to play.
    Cards(EpisodeCards<'a, V>),
    /// It doesn't: what Sonarr (or its metadata) lists, if anything.
    Lines(Vec<EpisodeLine>),
}

/// The episodes the library has of a season. There is at least one.
pub(super) struct EpisodeCards<'a, V> {
    pub count: usize,
    pub unwatched: usize,
    /// Makes the cards; only called when they're drawn.
    pub draw: DrawCards<'a, V>,
    /// A right click on the season's row, at this point of the window.
    pub menu: RowMenu<V>,
}

/// What Sonarr says about a season, and what can be asked of it.
pub(super) struct SonarrSay {
    pub status: String,
    pub color: u32,
    pub buttons: Vec<AnyElement>,
    /// The season's release list, when open.
    pub panel: Option<AnyElement>,
}

/// How many episodes a season has, for its row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Counts {
    /// In the library.
    have: usize,
    unwatched: usize,
    /// Listed by Sonarr or its metadata, when the library has none.
    listed: usize,
}

/// The words after a season's title. A season Sonarr speaks for that
/// isn't in the library has Sonarr's words instead.
fn summary(counts: Counts, sonarr_speaks: bool) -> Option<String> {
    match (counts.have, counts.unwatched) {
        (0, _) if counts.listed > 0 && !sonarr_speaks => Some(match counts.listed {
            1 => "1 episode".to_string(),
            n => format!("{n} episodes"),
        }),
        (0, _) => None,
        (1, 0) => Some("1 episode · watched".to_string()),
        (n, 0) => Some(format!("{n} episodes · all watched")),
        (1, _) => Some("1 episode".to_string()),
        (n, u) if u == n => Some(format!("{n} episodes")),
        (n, u) => Some(format!("{n} episodes · {u} unwatched")),
    }
}

/// The season's block: its row, its release list and its episodes.
pub(super) fn block<V: 'static>(
    host: &Host<'_, V>,
    season: Season<'_, V>,
    cx: &mut Context<V>,
) -> AnyElement {
    let Season {
        show,
        number,
        episodes,
        sonarr,
    } = season;
    let counts = match &episodes {
        Episodes::Cards(cards) => Counts {
            have: cards.count,
            unwatched: cards.unwatched,
            listed: 0,
        },
        Episodes::Lines(lines) => Counts {
            have: 0,
            unwatched: 0,
            listed: lines.len(),
        },
    };
    let have = counts.have > 0;
    let foldable = have || counts.listed > 0;
    let open = foldable && host.seasons.is_open(show, number);
    let (status, buttons, panel) = match sonarr {
        Some(say) => (Some((say.status, say.color)), say.buttons, say.panel),
        None => (None, Vec::new(), None),
    };
    let row = Row {
        show,
        number,
        summary: summary(counts, status.is_some()),
        status,
        have,
        foldable,
        open,
    };
    let (episodes, menu) = match episodes {
        Episodes::Cards(cards) => (Drawn::Cards(cards.draw), Some(cards.menu)),
        Episodes::Lines(lines) => (Drawn::Lines(lines), None),
    };
    let label = label(host, &row, menu, cx);
    // Sonarr's say about a season the library has comes in after it; a
    // season only Sonarr knows of comes in whole (below).
    let buttons: Vec<AnyElement> = if have {
        buttons
            .into_iter()
            .enumerate()
            .map(|(n, button)| {
                host.motion.arriving(
                    format!("season-button-{show}-{number}-{n}"),
                    div().flex_none().child(button),
                )
            })
            .collect()
    } else {
        buttons
    };
    let row_line = div()
        .flex()
        .items_center()
        .gap_3()
        .px_4()
        .rounded_lg()
        .bg(rgba(0xffffff0a))
        .hover(|s| s.bg(rgba(0xffffff12)))
        .child(label)
        .children(buttons);
    let grid = folding_episodes(host, &row, counts, episodes, cx);
    let block = div()
        .flex()
        .flex_col()
        .child(row_line)
        .children(panel.map(|panel| div().pt_2().child(panel)))
        .children(grid);
    if have {
        block.into_any_element()
    } else {
        host.motion
            .arriving(format!("season-{show}-{number}"), block)
    }
}

/// What a season's row says.
struct Row<'a> {
    show: &'a str,
    number: u16,
    summary: Option<String>,
    /// Sonarr's words and their colour.
    status: Option<(String, u32)>,
    /// The library has episodes of it.
    have: bool,
    foldable: bool,
    open: bool,
}

/// The episodes to draw under the row.
enum Drawn<'a, V> {
    Cards(DrawCards<'a, V>),
    Lines(Vec<EpisodeLine>),
}

/// The part of the row that opens and closes the season: its arrow, title,
/// episode count and Sonarr's words.
fn label<V: 'static>(
    host: &Host<'_, V>,
    row: &Row<'_>,
    menu: Option<RowMenu<V>>,
    cx: &mut Context<V>,
) -> AnyElement {
    let (show, number, have) = (row.show, row.number, row.have);
    let title = if number == 0 {
        "Specials".to_string()
    } else {
        format!("Season {number}")
    };
    let toggle = host.toggle;
    let toggle_show = show.to_string();
    div()
        .id(SharedString::from(format!("season-{number}")))
        .when_some(menu, |d, menu| {
            d.on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    menu(this, event.position, cx);
                }),
            )
        })
        .flex()
        .items_center()
        .gap_3()
        .flex_1()
        .min_w(px(0.0))
        .py_3()
        .when(row.foldable, |d| {
            d.cursor_pointer()
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    toggle(this, &toggle_show, number);
                    cx.notify();
                }))
        })
        .child(
            div()
                .w(px(14.0))
                .flex_none()
                .text_color(rgb(MUTED))
                .child(if !row.foldable {
                    ""
                } else if row.open {
                    "▾"
                } else {
                    "▸"
                }),
        )
        .child(
            div()
                .flex_none()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .children(
            row.summary
                .clone()
                .map(|s| div().flex_none().text_sm().text_color(rgb(MUTED)).child(s)),
        )
        .children(row.status.clone().map(|(text, color)| {
            let status = div()
                .min_w(px(0.0))
                .truncate()
                .text_sm()
                .text_color(rgb(color))
                .child(if have { format!("· {text}") } else { text });
            if have {
                host.motion
                    .arriving(format!("season-status-{show}-{number}"), status)
            } else {
                status.into_any_element()
            }
        }))
        .into_any_element()
}

/// The season's episodes, when open. Folding: they grow from nothing (or
/// shrink to it) while fading, as one motion over their real height
/// (measured each time they're drawn); a closing season stays drawn until
/// it's folded away. All spacing is inside the folding part, so it ends at
/// zero.
fn folding_episodes<V: 'static>(
    host: &Host<'_, V>,
    row: &Row<'_>,
    counts: Counts,
    episodes: Drawn<'_, V>,
    cx: &mut Context<V>,
) -> Option<AnyElement> {
    let (show, number) = (row.show, row.number);
    let fold = host.seasons.folding(show, number);
    let closing = fold.is_some_and(|fold| fold.closing);
    let shows = if row.have {
        seasons::Episodes::Cards(counts.have)
    } else {
        seasons::Episodes::Lines(counts.listed)
    };
    let height = host.seasons.height(show, number, shows);
    if !(row.open || (row.foldable && closing)) {
        return None;
    }
    let content = match episodes {
        Drawn::Lines(lines) => episode_list::episode_rows(&lines),
        Drawn::Cards(draw) => widgets::grid(draw(cx)),
    };
    let inner = div()
        .relative()
        .pt_2()
        .pb_4()
        .child(content)
        .child(host.seasons.height_marker(show, number));
    let outer = div().overflow_hidden().child(inner);
    let Some(fold) = fold else {
        return Some(outer.into_any_element());
    };
    let opening = fold.opening;
    let folding = outer.with_animation(
        ("fold", fold.key),
        Animation::new(crate::motion::FOLD.time()).with_easing(gpui::ease_in_out),
        move |outer, t| {
            let shown = seasons::shown(opening, t);
            if opening && t >= 1.0 {
                outer
            } else {
                outer.max_h(px(height * shown)).opacity(shown)
            }
        },
    );
    Some(folding.into_any_element())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(have: usize, unwatched: usize, listed: usize) -> Counts {
        Counts {
            have,
            unwatched,
            listed,
        }
    }

    #[test]
    fn a_season_in_the_library_counts_its_episodes_and_the_unwatched() {
        assert_eq!(
            summary(counts(1, 0, 0), false).as_deref(),
            Some("1 episode · watched")
        );
        assert_eq!(
            summary(counts(8, 0, 0), false).as_deref(),
            Some("8 episodes · all watched")
        );
        assert_eq!(summary(counts(1, 1, 0), true).as_deref(), Some("1 episode"));
        assert_eq!(
            summary(counts(8, 8, 0), false).as_deref(),
            Some("8 episodes")
        );
        assert_eq!(
            summary(counts(8, 3, 0), true).as_deref(),
            Some("8 episodes · 3 unwatched")
        );
    }

    #[test]
    fn a_listed_season_counts_its_lines_unless_sonarr_speaks_for_it() {
        assert_eq!(
            summary(counts(0, 0, 1), false).as_deref(),
            Some("1 episode")
        );
        assert_eq!(
            summary(counts(0, 0, 12), false).as_deref(),
            Some("12 episodes")
        );
        assert_eq!(summary(counts(0, 0, 12), true), None);
        assert_eq!(summary(counts(0, 0, 0), false), None);
    }
}
