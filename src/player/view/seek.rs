//! Drawing the seek bar: progress, what's buffered, marked parts, chapter
//! gaps, and the time preview under the mouse.

use gpui::{
    AnyElement, Context, Div, MouseButton, ObjectFit, canvas, div, img, prelude::*, px, relative,
    rgb, rgba,
};

use super::bar::TRACK;
use super::widgets::chip;
use super::{ACCENT, BORDER, SLIDER_HEIGHT, TEXT};
use crate::player::segments::{self, Chapter};
use crate::player::{Player, format_time};
use crate::video::Segment;

/// Intros, recaps and credits on the seek bar.
const SEGMENT: u32 = 0xffcf5c99;
/// The gaps that split the seek bar into chapters.
const CHAPTER_GAP: u32 = 0x000000b3;
/// What's buffered ahead, over the track.
const BUFFERED: u32 = 0xffffff38;
/// The hover preview's picture.
const PREVIEW_WIDTH: f32 = 240.0;
const PREVIEW_HEIGHT: f32 = 135.0;
/// Room for the preview's label, centred on the mouse.
const PREVIEW_LABEL_WIDTH: f32 = 320.0;
/// More chapters than this are usually fixed-interval markers: no ticks.
const MAX_CHAPTER_TICKS: usize = 40;
/// The track's height, and how much it grows under the mouse.
const TRACK_HEIGHT: f32 = 4.0;
const TRACK_GROWTH: f32 = 2.0;

/// Where `time` falls along a bar for a video `duration` long, 0 to 1.
fn fraction(time: f64, duration: f64) -> f32 {
    if duration > 0.0 {
        (time / duration).clamp(0.0, 1.0) as f32
    } else {
        0.0
    }
}

/// A marked part on the bar, as fractions of its width.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Band {
    start: f32,
    end: f32,
}

fn bands(segments: &[Segment], duration: f64) -> Vec<Band> {
    segments
        .iter()
        .map(|segment| Band {
            start: fraction(segment.start, duration),
            end: fraction(segment.end, duration),
        })
        .filter(|band| band.end > band.start)
        .collect()
}

/// Where the gaps between chapters go, as fractions of the bar's width.
fn chapter_ticks(chapters: &[Chapter], duration: f64) -> Vec<f32> {
    if !(2..=MAX_CHAPTER_TICKS).contains(&chapters.len()) {
        return Vec::new();
    }
    chapters
        .iter()
        .map(|chapter| fraction(chapter.start, duration))
        .filter(|at| *at > 0.002 && *at < 0.998)
        .collect()
}

/// The preview's label: the time, with the chapter's name if it has one.
fn preview_label(time: f64, chapters: &[Chapter]) -> String {
    let chapter = segments::chapter_at(chapters, time)
        .and_then(|chapter| segments::chapter_label(&chapter.title));
    match chapter {
        Some(chapter) => format!("{} · {chapter}", format_time(time)),
        None => format_time(time),
    }
}

/// A stretch of the track from its left end, `width` (0 to 1) of it.
fn filled(width: f32) -> Div {
    div()
        .absolute()
        .top_0()
        .left_0()
        .h_full()
        .w(relative(width))
        .rounded_full()
}

impl Player {
    pub(super) fn seek_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let duration = self.status.duration;
        let seek = &self.sliders.seek;
        let progress = fraction(seek.shown(self.status.position), duration);
        // The time preview fades in under the mouse, and the bar thickens
        // with it.
        let hover = self.fades.preview.value();
        let bounds = seek.bounds_cell();
        div()
            .id("seek")
            .flex_1()
            .min_w(px(80.0))
            .h(px(SLIDER_HEIGHT))
            .relative()
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_seek_down))
            .child(
                canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .child(self.seek_track(progress, hover))
            .child(
                div()
                    .absolute()
                    .top(px((SLIDER_HEIGHT - 12.0) / 2.0))
                    .left(relative(progress))
                    .ml(px(-6.0))
                    .size(px(12.0))
                    .rounded_full()
                    .bg(rgb(TEXT)),
            )
            .children(self.seek_preview(hover))
            .into_any_element()
    }

    /// The track: what's buffered, the progress, marked parts and chapter gaps.
    fn seek_track(&self, progress: f32, hover: f32) -> Div {
        let duration = self.status.duration;
        let track_height = TRACK_HEIGHT + TRACK_GROWTH * hover;
        let buffered = self.status.buffered.map(|time| fraction(time, duration));
        div()
            .absolute()
            .left_0()
            .right_0()
            .top(px((SLIDER_HEIGHT - track_height) / 2.0))
            .h(px(track_height))
            .rounded_full()
            .bg(rgba(TRACK))
            .children(buffered.map(|width| filled(width).bg(rgba(BUFFERED))))
            .child(filled(progress).bg(rgb(ACCENT)))
            .children(
                bands(self.markers.segments(), duration)
                    .into_iter()
                    .map(|band| {
                        div()
                            .absolute()
                            .top_0()
                            .h_full()
                            .left(relative(band.start))
                            .w(relative(band.end - band.start))
                            .bg(rgba(SEGMENT))
                    }),
            )
            .children(
                chapter_ticks(self.markers.chapters(), duration)
                    .into_iter()
                    .map(|at| {
                        div()
                            .absolute()
                            .top_0()
                            .h_full()
                            .left(relative(at))
                            .ml(px(-1.0))
                            .w(px(2.0))
                            .bg(rgba(CHAPTER_GAP))
                    }),
            )
    }

    /// The time (and picture) under the mouse, while it's on screen.
    fn seek_preview(&self, hover: f32) -> Option<Div> {
        if !self.fades.preview.on_screen() {
            return None;
        }
        let seek = &self.sliders.seek;
        let x = seek.preview_x();
        let area = seek.bounds();
        let time = seek.time_at(x, self.status.duration);
        let offset = f32::from(x - area.origin.x).clamp(0.0, f32::from(area.size.width));
        let picture = seek.previews.as_ref().map(|previews| {
            // Black until the first picture arrives.
            div()
                .w(px(PREVIEW_WIDTH))
                .h(px(PREVIEW_HEIGHT))
                .rounded_lg()
                .overflow_hidden()
                .bg(rgb(0x000000))
                .border_1()
                .border_color(rgba(BORDER))
                .children(
                    previews
                        .frame_for(time)
                        .map(|path| img(path).size_full().object_fit(ObjectFit::Contain)),
                )
        });
        let preview = div()
            .absolute()
            .opacity(hover)
            .bottom(px(SLIDER_HEIGHT + 6.0))
            .left(px(offset - PREVIEW_LABEL_WIDTH / 2.0))
            .w(px(PREVIEW_LABEL_WIDTH))
            .flex()
            .flex_col()
            .items_center()
            .gap_1p5()
            .children(picture)
            .child(
                chip()
                    .max_w(px(PREVIEW_LABEL_WIDTH))
                    .px_2()
                    .py_0p5()
                    .text_xs()
                    .truncate()
                    .child(preview_label(time, self.markers.chapters())),
            );
        Some(preview)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::SegmentKind;

    fn chapters(starts: &[f64]) -> Vec<Chapter> {
        starts
            .iter()
            .map(|&start| Chapter {
                title: format!("Chapter {start}"),
                start,
            })
            .collect()
    }

    #[test]
    fn a_time_is_a_fraction_of_the_length() {
        assert_eq!(fraction(250.0, 1000.0), 0.25);
        assert_eq!(fraction(1200.0, 1000.0), 1.0);
        assert_eq!(fraction(-5.0, 1000.0), 0.0);
        assert_eq!(fraction(250.0, 0.0), 0.0);
    }

    #[test]
    fn parts_with_no_width_are_not_drawn() {
        let segments = [
            Segment {
                kind: SegmentKind::Intro,
                start: 100.0,
                end: 200.0,
            },
            Segment {
                kind: SegmentKind::Credits,
                start: 1500.0,
                end: 1600.0,
            },
        ];
        assert_eq!(
            bands(&segments, 1000.0),
            [Band {
                start: 0.1,
                end: 0.2
            }]
        );
    }

    #[test]
    fn chapter_gaps_skip_the_ends_of_the_bar() {
        let ticks = chapter_ticks(&chapters(&[0.0, 250.0, 500.0, 999.5]), 1000.0);
        assert_eq!(ticks, [0.25, 0.5]);
    }

    #[test]
    fn one_chapter_or_too_many_make_no_gaps() {
        assert!(chapter_ticks(&chapters(&[0.0]), 1000.0).is_empty());
        let many: Vec<f64> = (0..41).map(|i| i as f64 * 20.0).collect();
        assert!(chapter_ticks(&chapters(&many), 1000.0).is_empty());
    }

    #[test]
    fn the_preview_names_the_chapter_unless_it_is_a_placeholder() {
        let named = [Chapter {
            title: "Opening".into(),
            start: 0.0,
        }];
        assert_eq!(preview_label(65.0, &named), "1:05 · Opening");
        assert_eq!(preview_label(65.0, &chapters(&[0.0])), "1:05");
        assert_eq!(preview_label(65.0, &[]), "1:05");
    }
}
