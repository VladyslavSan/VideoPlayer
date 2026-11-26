//! The marked parts of the video: intros, recaps and credits from every
//! source, and the file's chapters. `segments` has the rules for finding them.

use super::segments::{self, Chapter};
use crate::video::{Segment, SegmentKind};

/// Credits that start after this fraction of the video are its end credits.
const END_CREDITS_FROM: f64 = 0.7;
/// A mark this close to the end (or to another mark) isn't worth a jump, in seconds.
const MARK_SPACING: f64 = 1.0;
/// Going back from less than this far into a part goes to the mark before it.
const BACK_SLACK: f64 = 3.0;

/// Where intro/credits markers came from, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentSource {
    /// The Jellyfin server's media segments.
    Server,
    /// TheIntroDB / IntroDB.
    Online,
    /// Chapter names in the file.
    Chapters,
}

/// What the skip button offers: its label and where it jumps to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct SkipTarget {
    pub(super) label: &'static str,
    pub(super) end: f64,
}

/// A place worth jumping to: the start of a part or chapter, or a part's end.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Mark {
    pub(super) at: f64,
    pub(super) name: String,
}

/// Which way a jump between marks goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Direction {
    Back,
    Forward,
}

/// Intros, recaps, credits and chapters of the video being played.
pub(super) struct Markers {
    /// Intros, recaps and credits, combined from `sources`.
    segments: Vec<Segment>,
    /// What each `SegmentSource` found, best first.
    sources: [Vec<Segment>; 3],
    chapters_read: bool,
    chapters: Vec<Chapter>,
}

impl Markers {
    pub(super) fn new() -> Self {
        Self {
            segments: Vec::new(),
            sources: Default::default(),
            chapters_read: false,
            chapters: Vec::new(),
        }
    }

    /// Takes what `source` found. For each kind, the best source that has it wins.
    pub(super) fn add(&mut self, source: SegmentSource, found: Vec<Segment>) {
        self.sources[source as usize] = found;
        let sources: Vec<&[Segment]> = self.sources.iter().map(Vec::as_slice).collect();
        let merged = segments::merge(&sources);
        if merged != self.segments {
            eprintln!("[player] segments: {merged:?}");
            self.segments = merged;
        }
    }

    pub(super) fn segments(&self) -> &[Segment] {
        &self.segments
    }

    pub(super) fn chapters(&self) -> &[Chapter] {
        &self.chapters
    }

    /// Whether the file's chapters are still to be read (once its length is known).
    pub(super) fn wants_chapters(&self, duration: f64) -> bool {
        !self.chapters_read && duration > 0.0
    }

    /// Takes the file's chapters, and the parts their names give away.
    pub(super) fn set_chapters(&mut self, chapters: Vec<Chapter>, duration: f64) {
        self.chapters_read = true;
        self.chapters = chapters;
        let found = segments::from_chapters(&self.chapters, duration);
        self.add(SegmentSource::Chapters, found);
    }

    fn segment_at(&self, position: f64) -> Option<Segment> {
        self.segments
            .iter()
            .find(|s| s.start <= position && position < s.end)
            .copied()
    }

    pub(super) fn in_credits(&self, position: f64) -> bool {
        self.segment_at(position)
            .is_some_and(|s| s.kind == SegmentKind::Credits)
    }

    /// Whether the end credits have started (opening credits don't count).
    pub(super) fn end_credits_started(&self, position: f64, duration: f64) -> bool {
        self.segments.iter().any(|s| {
            s.kind == SegmentKind::Credits
                && s.start >= duration * END_CREDITS_FROM
                && position >= s.start
        })
    }

    /// What the skip button offers at `position`, while a part can be skipped.
    pub(super) fn skip_target(&self, position: f64) -> Option<SkipTarget> {
        let segment = self.segment_at(position)?;
        let label = match segment.kind {
            SegmentKind::Intro => "Skip Intro",
            SegmentKind::Recap => "Skip Recap",
            SegmentKind::Preview => "Skip Preview",
            SegmentKind::Credits => return None,
        };
        // Not in the last second, where skipping would barely move.
        (position < segment.end - 1.0).then_some(SkipTarget {
            label,
            end: segment.end,
        })
    }

    /// Every mark of a video `duration` long, in order (0: length not known).
    fn marks(&self, duration: f64) -> Vec<Mark> {
        let mut marks = vec![Mark {
            at: 0.0,
            name: "The beginning".to_string(),
        }];
        for segment in &self.segments {
            let name = match segment.kind {
                SegmentKind::Intro => "intro",
                SegmentKind::Recap => "recap",
                SegmentKind::Preview => "preview",
                SegmentKind::Credits => "credits",
            };
            let mut capital = name.to_string();
            capital[..1].make_ascii_uppercase();
            marks.push(Mark {
                at: segment.start,
                name: capital,
            });
            if segment.kind != SegmentKind::Credits {
                marks.push(Mark {
                    at: segment.end,
                    name: format!("After the {name}"),
                });
            }
        }
        for chapter in &self.chapters {
            let name = if chapter.title.trim().is_empty() {
                "Chapter".to_string()
            } else {
                chapter.title.clone()
            };
            marks.push(Mark {
                at: chapter.start,
                name,
            });
        }
        marks.retain(|mark| duration <= 0.0 || mark.at < duration - MARK_SPACING);
        marks.sort_by(|a, b| a.at.total_cmp(&b.at));
        marks.dedup_by(|a, b| (a.at - b.at).abs() < MARK_SPACING);
        marks
    }

    /// The next mark after `position`, or the one to go back to: a moment
    /// into a part that is its start; right at it, the mark before.
    pub(super) fn mark_from(
        &self,
        position: f64,
        duration: f64,
        direction: Direction,
    ) -> Option<Mark> {
        let mut marks = self.marks(duration).into_iter();
        match direction {
            Direction::Forward => marks.find(|mark| mark.at > position + MARK_SPACING),
            Direction::Back => marks.rev().find(|mark| mark.at < position - BACK_SLACK),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(kind: SegmentKind, start: f64, end: f64) -> Segment {
        Segment { kind, start, end }
    }

    /// An episode with a recap, an intro, three chapters and end credits.
    fn episode() -> Markers {
        let mut markers = Markers::new();
        markers.add(
            SegmentSource::Server,
            vec![
                segment(SegmentKind::Recap, 0.0, 30.0),
                segment(SegmentKind::Intro, 30.0, 120.0),
                segment(SegmentKind::Credits, 1300.0, 1400.0),
            ],
        );
        markers.chapters = Chapter::list(vec![
            ("".to_string(), 0.0),
            ("Part A".to_string(), 120.5),
            ("Part B".to_string(), 700.0),
        ]);
        markers
    }

    #[test]
    fn a_better_source_wins_for_each_kind() {
        let mut markers = Markers::new();
        markers.add(
            SegmentSource::Chapters,
            vec![
                segment(SegmentKind::Intro, 10.0, 100.0),
                segment(SegmentKind::Credits, 1300.0, 1400.0),
            ],
        );
        markers.add(
            SegmentSource::Server,
            vec![segment(SegmentKind::Intro, 12.0, 95.0)],
        );
        assert_eq!(
            markers.segments(),
            [
                segment(SegmentKind::Intro, 12.0, 95.0),
                segment(SegmentKind::Credits, 1300.0, 1400.0),
            ]
        );
    }

    #[test]
    fn the_skip_button_names_the_part_and_jumps_to_its_end() {
        let markers = episode();
        assert_eq!(
            markers.skip_target(10.0),
            Some(SkipTarget {
                label: "Skip Recap",
                end: 30.0
            })
        );
        assert_eq!(
            markers.skip_target(60.0).map(|t| t.label),
            Some("Skip Intro")
        );
        assert_eq!(markers.skip_target(500.0), None);
    }

    #[test]
    fn nothing_is_skipped_in_a_parts_last_second_or_in_the_credits() {
        let markers = episode();
        assert_eq!(markers.skip_target(119.5), None);
        assert_eq!(markers.skip_target(1350.0), None);
        assert!(markers.in_credits(1350.0));
        assert!(!markers.in_credits(1400.0));
    }

    #[test]
    fn only_closing_credits_finish_the_video() {
        let mut markers = Markers::new();
        markers.add(
            SegmentSource::Server,
            vec![
                segment(SegmentKind::Credits, 60.0, 120.0),
                segment(SegmentKind::Credits, 1300.0, 1400.0),
            ],
        );
        assert!(!markers.end_credits_started(90.0, 1400.0));
        assert!(!markers.end_credits_started(1299.0, 1400.0));
        assert!(markers.end_credits_started(1300.0, 1400.0));
    }

    #[test]
    fn marks_are_sorted_and_close_ones_merge() {
        let names: Vec<String> = episode()
            .marks(1400.0)
            .into_iter()
            .map(|mark| mark.name)
            .collect();
        assert_eq!(
            names,
            [
                "The beginning",
                "After the recap",
                "After the intro",
                "Part B",
                "Credits"
            ]
        );
    }

    #[test]
    fn marks_in_the_last_second_are_dropped_when_the_length_is_known() {
        let mut markers = Markers::new();
        markers.chapters = Chapter::list(vec![("End".to_string(), 1399.5)]);
        assert_eq!(markers.marks(1400.0).len(), 1);
        assert_eq!(markers.marks(0.0).len(), 2);
    }

    #[test]
    fn going_forward_finds_the_next_mark() {
        let markers = episode();
        let next = markers.mark_from(40.0, 1400.0, Direction::Forward);
        assert_eq!(next.map(|mark| mark.at), Some(120.0));
        assert_eq!(markers.mark_from(1350.0, 1400.0, Direction::Forward), None);
    }

    #[test]
    fn going_back_finds_the_parts_start_or_the_one_before() {
        let markers = episode();
        let at = |position| {
            markers
                .mark_from(position, 1400.0, Direction::Back)
                .map(|mark| mark.at)
        };
        // Well into Part B: its start. Right at its start: the mark before.
        assert_eq!(at(900.0), Some(700.0));
        assert_eq!(at(701.0), Some(120.0));
        assert_eq!(at(2.0), None);
    }
}
