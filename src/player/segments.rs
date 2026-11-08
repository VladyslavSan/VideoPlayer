//! Intros, recaps and credits found from chapter names, for files the
//! server hasn't analysed (or local files).

use crate::video::{Segment, SegmentKind};

/// What a chapter called `title` is, judging by common naming
/// ("Opening", "OP", "Intro", "Previously on…", "End Credits", "ED").
pub fn kind_of_chapter(title: &str) -> Option<SegmentKind> {
    let lower = title.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let has = |options: &[&str]| words.iter().any(|w| options.contains(w));
    // Order matters: "Opening Credits" is an intro, "Preview of the next
    // episode" is a preview, not credits.
    if has(&["recap", "previously"]) {
        Some(SegmentKind::Recap)
    } else if has(&["intro", "opening", "op", "op1", "op2", "theme"]) {
        Some(SegmentKind::Intro)
    } else if has(&["preview", "trailer"]) {
        Some(SegmentKind::Preview)
    } else if has(&["credits", "credit", "ending", "outro", "ed", "ed1", "ed2"]) {
        Some(SegmentKind::Credits)
    } else {
        None
    }
}

/// Segments for recognised chapters; each runs until the next chapter
/// (or the end of the video).
pub fn from_chapters(chapters: &[(String, f64)], duration: f64) -> Vec<Segment> {
    chapters
        .iter()
        .enumerate()
        .filter_map(|(i, (title, start))| {
            let kind = kind_of_chapter(title)?;
            let end = chapters.get(i + 1).map_or(duration, |(_, t)| *t);
            (end > *start).then_some(Segment {
                kind,
                start: *start,
                end,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chapter_names() {
        let k = kind_of_chapter;
        assert_eq!(k("Opening"), Some(SegmentKind::Intro));
        assert_eq!(k("OP"), Some(SegmentKind::Intro));
        assert_eq!(k("Intro"), Some(SegmentKind::Intro));
        assert_eq!(k("Opening Credits"), Some(SegmentKind::Intro));
        assert_eq!(k("Previously on..."), Some(SegmentKind::Recap));
        assert_eq!(k("End Credits"), Some(SegmentKind::Credits));
        assert_eq!(k("ED"), Some(SegmentKind::Credits));
        assert_eq!(k("Ending"), Some(SegmentKind::Credits));
        assert_eq!(k("Preview"), Some(SegmentKind::Preview));
        assert_eq!(k("Chapter 3"), None);
        assert_eq!(k("Operation Overlord"), None);
        assert_eq!(k("The Edge"), None);
    }

    #[test]
    fn chapters_become_segments() {
        let chapters = vec![
            ("Prologue".to_string(), 0.0),
            ("Opening".to_string(), 90.0),
            ("Part A".to_string(), 180.0),
            ("Ending".to_string(), 1300.0),
        ];
        assert_eq!(
            from_chapters(&chapters, 1420.0),
            vec![
                Segment {
                    kind: SegmentKind::Intro,
                    start: 90.0,
                    end: 180.0
                },
                Segment {
                    kind: SegmentKind::Credits,
                    start: 1300.0,
                    end: 1420.0
                },
            ]
        );
    }
}
