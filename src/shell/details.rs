//! What show and movie pages have in common, whichever view draws them:
//! the extra information a server has for one (`Details`), the wide image
//! with the title over it at the top, and the overview under it.

use std::path::PathBuf;

use gpui::{
    Animation, AnimationExt, AnyElement, App, FontWeight, ObjectFit, SharedString, Window, div,
    img, linear_color_stop, linear_gradient, prelude::*, px, rgb, rgba,
};

use super::page_motion::Motion;
use super::{BG, MUTED};
use crate::jellyfin::ItemInfo;

/// Lower on a show's page, so its seasons are in view.
pub(super) const SHOW_HERO_HEIGHT: f32 = 280.0;

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
    pub(super) fn from_info(info: &ItemInfo, backdrop: Option<PathBuf>) -> Self {
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

/// Those of `images` loaded and ready to draw, in order. The rest are
/// asked for (the page is drawn again when they're ready), so each
/// fades in once it can be seen rather than finishing its fade before.
pub(super) fn ready_pictures(
    images: Vec<PathBuf>,
    window: &mut Window,
    cx: &mut App,
) -> Vec<PathBuf> {
    images
        .into_iter()
        .filter(|path| {
            let source = gpui::Resource::Path(path.as_path().into());
            window
                .use_asset::<gpui::ImgResourceLoader>(&source, cx)
                .is_some_and(|loaded| loaded.is_ok())
        })
        .collect()
}

/// The top of a page: a wide image with the title and facts over it.
pub(super) struct Hero {
    pub height: f32,
    /// Bottom to top: a small picture first, the full backdrop once fetched.
    pub images: Vec<PathBuf>,
    pub title: String,
    pub facts: Vec<String>,
    pub tagline: Option<String>,
}

/// A wide image with the title and facts over a fade to the background.
/// The images go bottom to top, each fading in over the ones below when
/// it arrives, so one never cuts to another.
pub(super) fn hero(motion: &Motion, hero: Hero) -> AnyElement {
    let Hero {
        height,
        images,
        title,
        facts,
        tagline,
    } = hero;
    let layers = images.into_iter().map(|path| {
        let id = SharedString::from(format!("hero-{}", path.display()));
        let layer = div()
            .absolute()
            .inset_0()
            .child(img(path).size_full().object_fit(ObjectFit::Cover));
        if crate::motion::enabled() {
            layer
                .with_animation(
                    id,
                    Animation::new(crate::motion::PICTURE.time())
                        .with_easing(crate::motion::ease_in_out),
                    |layer, t| layer.opacity(t),
                )
                .into_any_element()
        } else {
            layer.into_any_element()
        }
    });
    div()
        .relative()
        .w_full()
        .h(px(height))
        .mt_2()
        .rounded_2xl()
        .overflow_hidden()
        .bg(rgb(0x1a1d26))
        .children(layers)
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
                .child(div().text_3xl().font_weight(FontWeight::BOLD).child(title))
                // Details arriving add to the facts and bring a tagline:
                // the facts dissolve into the longer ones, and the room
                // for the tagline opens up (the title rising) as it
                // fades in.
                .when(!facts.is_empty(), |d| {
                    d.child(
                        motion.changing_text("hero-facts", facts.join("  ·  "), |facts| {
                            div()
                                .pt_2()
                                .text_sm()
                                .text_color(rgb(0xc9ceda))
                                .child(facts)
                        }),
                    )
                })
                .child(
                    motion.resizing(
                        "hero-tagline",
                        match tagline {
                            Some(tagline) => motion.arriving(
                                "hero-tagline",
                                div()
                                    .pt_2()
                                    .text_sm()
                                    .italic()
                                    .text_color(rgb(MUTED))
                                    .child(tagline),
                            ),
                            None => div().into_any_element(),
                        },
                    ),
                ),
        )
        .into_any_element()
}

/// An item's overview, as a page shows it.
#[derive(Default)]
pub(super) struct Overview<'a> {
    pub details: Option<&'a Details>,
    /// The details are being fetched: a note says so until they're here.
    pub loading: bool,
    /// The most lines shown (a show's page keeps it short so the seasons
    /// follow soon); None for all of it.
    pub lines: Option<usize>,
}

/// The item's overview, if it has one or it's on its way.
pub(super) fn overview(motion: &Motion, overview: Overview<'_>) -> Option<AnyElement> {
    let Overview {
        details,
        loading,
        lines,
    } = overview;
    let paragraph = |text: SharedString, color: u32| {
        div()
            .pt_6()
            .max_w(px(780.0))
            .line_height(px(24.0))
            .text_color(rgb(color))
            .when_some(lines, |d, lines| d.line_clamp(lines))
            .child(text)
    };
    let note = || paragraph("Loading details…".into(), MUTED);
    // From one line to several: the box grows to fit, not in one jump.
    let text = match details.and_then(|d| d.overview.clone()) {
        Some(text) => motion.arriving_over(
            "overview-loading",
            note(),
            "overview",
            paragraph(text.into(), 0xd5d9e3),
        ),
        None if loading => motion.waiting("overview-loading", note()),
        None => return None,
    };
    Some(motion.resizing("overview-box", text))
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
