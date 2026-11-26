//! The bar at the top of the library's pages: the search field or a way
//! back, the library's status, Refresh and Settings. It floats over the
//! page, which scrolls on underneath it, blurred (frosted) or fading out.

use gpui::{
    AnyElement, Bounds, Context, Div, FontWeight, SharedString, div, point, prelude::*, px, rgb,
    rgba,
};

use super::{BG, ERROR, Grid, MUTED, Screen, Shell, browse, widgets};

/// The bar at the top (search, Refresh, Settings). It floats over the page,
/// which scrolls on underneath it, seen through the bar.
pub(super) const HEADER_HEIGHT: f32 = 80.0;
/// The bar is near solid at the top and fades out to nothing at its
/// bottom edge (or at the sort bar's, on a library grid).
const BAR_TOP_OPACITY: f32 = 0.97;
/// Where the bar starts to fade, as a share of the header's height.
const BAR_FADE_START: f32 = 0.3;
/// The opacity where the header meets a sort bar below it.
pub(super) const BAR_JOIN_OPACITY: f32 = 0.6;

/// Frosted glass: the page is blurred under the bars, under a light tint.
/// Only GPUI's Windows renderer blurs so far; elsewhere the bars fade.
pub(super) const FROSTED: bool = cfg!(target_os = "windows");
/// How soft the page under the bars gets (the blur's standard deviation).
const FROST_BLUR: f32 = 20.0;
/// The tint over the blur.
const FROST_TINT: f32 = 0.4;
/// The faint line under frosted bars.
pub(super) const FROST_EDGE: u32 = 0xffffff12;

/// The bars' own background: a light tint over the frost, or a fade.
pub(super) fn bar_background(start: f32, from: f32, to: f32) -> gpui::Background {
    if FROSTED {
        bar_tint(FROST_TINT).into()
    } else {
        bar_fade(start, from, to)
    }
}

/// The page colour at `opacity`, for the fading bar.
fn bar_tint(opacity: f32) -> gpui::Hsla {
    let mut color: gpui::Hsla = rgb(BG).into();
    color.a = opacity;
    color
}

/// A top-to-bottom fade of the page colour, `from` (solid until `start`,
/// a share of the height) to `to`.
fn bar_fade(start: f32, from: f32, to: f32) -> gpui::Background {
    gpui::linear_gradient(
        180.0,
        gpui::linear_color_stop(bar_tint(from), start),
        gpui::linear_color_stop(bar_tint(to), 1.0),
    )
}

/// A control on the see-through bar, on a solid backing of the page's
/// colour so it looks the same whatever passes underneath.
fn solid(control: AnyElement) -> AnyElement {
    div()
        .flex_none()
        .rounded_md()
        .bg(rgb(BG))
        .child(control)
        .into_any_element()
}

/// A line of the library's status in the bar; it gives way (truncated)
/// before the buttons do.
fn note(text: impl IntoElement, color: u32) -> Div {
    div()
        .min_w(px(0.0))
        .text_sm()
        .text_color(rgb(color))
        .truncate()
        .child(text)
}

impl Shell {
    /// The search field, in the header over the library.
    fn search_field(&self) -> AnyElement {
        div()
            .w(px(320.0))
            .flex_none()
            .child(self.search.clone())
            .into_any_element()
    }

    /// Whether a library grid's sort bar sits under the header.
    pub(super) fn sort_bar_below(&self, cx: &Context<Self>) -> bool {
        (Grid::of(self.navigation.screen()).is_some()
            || matches!(self.navigation.screen(), Screen::Arr(..)))
            && self.search_query(cx).is_empty()
    }

    /// The blur under the header (and the sort bar, if there is one): one
    /// layer, drawn after the page and before either bar, so each bar's
    /// buttons stay out of the other's blur.
    pub(super) fn frost(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if !FROSTED {
            return None;
        }
        let sort_bar = self.sort_bar_below(cx);
        let height = HEADER_HEIGHT
            + if sort_bar {
                browse::SORT_BAR_HEIGHT
            } else {
                0.0
            };
        let frost = gpui::canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let blur = |rect| {
                    gpui::quad(
                        rect,
                        px(0.0),
                        gpui::backdrop_blur(px(FROST_BLUR)),
                        px(0.0),
                        gpui::transparent_black(),
                        gpui::BorderStyle::default(),
                    )
                };
                let header = Bounds::new(
                    bounds.origin,
                    gpui::size(bounds.size.width, px(HEADER_HEIGHT)),
                );
                // One layer: both blurs are drawn together, from the page
                // as it was, so there's no seam where they meet. It reaches
                // as far as the blur reads, so it comes after all of that.
                window.paint_layer(bounds.dilate(px(FROST_BLUR * 3.0)), |window| {
                    window.paint_quad(blur(header));
                    if sort_bar {
                        // The sort bar spans the grid, beside the sidebar.
                        let left = px(browse::SIDEBAR_WIDTH);
                        window.paint_quad(blur(Bounds::new(
                            point(bounds.origin.x + left, bounds.origin.y + px(HEADER_HEIGHT)),
                            gpui::size(bounds.size.width - left, px(browse::SORT_BAR_HEIGHT)),
                        )));
                    }
                });
            },
        );
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(px(height))
                .child(frost.size_full())
                .into_any_element(),
        )
    }

    /// The bar at the top: the search field or a way back, then the
    /// library's status and the buttons.
    pub(super) fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        // On a library grid the sort bar carries the fade on below.
        let sort_bar_below = self.sort_bar_below(cx);
        let fades_to = if sort_bar_below {
            BAR_JOIN_OPACITY
        } else {
            0.0
        };
        div()
            .id("header")
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(HEADER_HEIGHT))
            .bg(bar_background(BAR_FADE_START, BAR_TOP_OPACITY, fades_to))
            .when(FROSTED && !sort_bar_below, |bar| {
                bar.border_b_1().border_color(rgba(FROST_EDGE))
            })
            // Clicks on the bar itself don't reach the page under it.
            .occlude()
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .px_8()
            .child(div().flex_none().child(self.header_title(cx)))
            .child(self.header_status(cx))
            .into_any_element()
    }

    /// The left of the bar: the search field on a tab, a way back on a
    /// page opened from one.
    fn header_title(&self, cx: &mut Context<Self>) -> AnyElement {
        let heading =
            |text: SharedString| div().text_2xl().font_weight(FontWeight::BOLD).child(text);
        let back = |cx: &mut Context<Self>| {
            solid(widgets::button(
                "home",
                "‹ Library",
                cx,
                |this, window, cx| this.leave_page(window, cx),
            ))
        };
        match self.navigation.screen() {
            Screen::Home
            | Screen::Source(_)
            | Screen::Movies(_)
            | Screen::Shows(_)
            | Screen::Downloads
            | Screen::Arr(..) => solid(self.search_field()),
            Screen::Show(_) | Screen::Details(_) | Screen::Tracked(..) | Screen::Found(..) => {
                back(cx)
            }
            Screen::Settings => div()
                .flex()
                .items_center()
                .gap_3()
                .child(back(cx))
                .child(heading("Settings".into()))
                .into_any_element(),
        }
    }

    /// The right of the bar: what the library is doing, what went wrong,
    /// Refresh and Settings.
    fn header_status(&self, cx: &mut Context<Self>) -> AnyElement {
        let (status, error) = {
            let library = self.library.read(cx);
            (library.status(), library.error().cloned())
        };
        let can_refresh = self.library.read(cx).catalog().has_sources();
        div()
            .flex()
            .flex_1()
            .min_w(px(0.0))
            .justify_end()
            .items_center()
            .gap_3()
            .children(status.map(|status| note(status, MUTED)))
            .children(error.map(|error| note(error, ERROR)))
            .children(can_refresh.then(|| {
                solid(widgets::button("refresh", "Refresh", cx, |this, _, cx| {
                    this.refresh(cx)
                }))
            }))
            .children((*self.navigation.screen() != Screen::Settings).then(|| {
                solid(widgets::button(
                    "settings",
                    "Settings",
                    cx,
                    |this, window, cx| this.open_settings(window, cx),
                ))
            }))
            .into_any_element()
    }

    /// Has the library look for changes in its folders and on its servers.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.library.update(cx, |library, cx| library.refresh(cx));
    }
}
