//! Intros, recaps and credits found from chapter names, for files the
//! server hasn't analysed (or local files), and combining them with
//! markers from elsewhere.

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
    // Japanese has no spaces between words ("次回予告", next-episode preview).
    let contains = |options: &[&str]| options.iter().any(|o| lower.contains(o));
    // Order matters: "Opening Credits" is an intro, "Preview of the next
    // episode" is a preview, not credits.
    // Japanese releases name them in katakana: オープニング (opening),
    // エンディング (ending), 予告 (preview).
    if has(&["recap", "previously"]) {
        Some(SegmentKind::Recap)
    } else if has(&["intro", "opening", "op", "op1", "op2", "theme"]) || contains(&["オープニング"])
    {
        Some(SegmentKind::Intro)
    } else if has(&["preview", "trailer"]) || contains(&["予告"]) {
        Some(SegmentKind::Preview)
    } else if has(&["credits", "credit", "ending", "outro", "ed", "ed1", "ed2"])
        || contains(&["エンディング"])
    {
        Some(SegmentKind::Credits)
    } else {
        None
    }
}

/// A chapter's name worth showing, or None for numbered placeholders
/// ("Chapter 3", "Kapitel 03", "00:05:00.000", "4").
pub fn chapter_label(title: &str) -> Option<&str> {
    let title = title.trim();
    let lower = title.to_lowercase();
    let rest = [
        "chapter",
        "chap.",
        "ch.",
        "kapitel",
        "chapitre",
        "capítulo",
        "capitulo",
        "capitolo",
        "hoofdstuk",
        "rozdział",
        "глава",
        "розділ",
    ]
    .iter()
    .find_map(|word| lower.strip_prefix(word))
    .unwrap_or(&lower);
    let placeholder = rest
        .trim()
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, ':' | '.' | ' '));
    (!placeholder).then_some(title)
}

/// The chapter playing at `time`: the last one starting at or before it.
pub fn chapter_at(chapters: &[(String, f64)], time: f64) -> Option<&(String, f64)> {
    chapters.iter().rev().find(|(_, start)| *start <= time)
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

/// Combines segments from several sources, best first: each kind (intro,
/// credits, ...) comes from the first source that has it. Sorted by start.
pub fn merge(sources: &[&[Segment]]) -> Vec<Segment> {
    let mut merged: Vec<Segment> = Vec::new();
    for source in sources {
        let have: Vec<SegmentKind> = merged.iter().map(|s| s.kind).collect();
        merged.extend(source.iter().filter(|s| !have.contains(&s.kind)));
    }
    merged.sort_by(|a, b| a.start.total_cmp(&b.start));
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merging_prefers_earlier_sources_per_kind() {
        let seg = |kind, start, end| Segment { kind, start, end };
        let server = [seg(SegmentKind::Intro, 60.0, 120.0)];
        let online = [
            seg(SegmentKind::Credits, 3000.0, 3100.0),
            seg(SegmentKind::Intro, 61.0, 121.0),
            seg(SegmentKind::Recap, 0.0, 55.0),
        ];
        let chapters = [seg(SegmentKind::Credits, 2990.0, 3100.0)];
        assert_eq!(
            merge(&[&server, &online, &chapters]),
            vec![
                seg(SegmentKind::Recap, 0.0, 55.0),
                seg(SegmentKind::Intro, 60.0, 120.0),
                seg(SegmentKind::Credits, 3000.0, 3100.0),
            ]
        );
    }

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
        assert_eq!(k("オープニング"), Some(SegmentKind::Intro));
        assert_eq!(k("エンディング"), Some(SegmentKind::Credits));
        assert_eq!(k("次回予告"), Some(SegmentKind::Preview));
        assert_eq!(k("予告"), Some(SegmentKind::Preview));
        assert_eq!(k("Aパート"), None);
    }

    #[test]
    fn placeholder_chapter_names_are_hidden() {
        assert_eq!(chapter_label("Chapter 3"), None);
        assert_eq!(chapter_label("chapter 03"), None);
        assert_eq!(chapter_label("Kapitel 12"), None);
        assert_eq!(chapter_label("00:05:00.000"), None);
        assert_eq!(chapter_label("4"), None);
        assert_eq!(chapter_label("  "), None);
        assert_eq!(
            chapter_label("Chapter 3: The Heist"),
            Some("Chapter 3: The Heist")
        );
        assert_eq!(chapter_label("Opening"), Some("Opening"));
        assert_eq!(chapter_label("Bパート"), Some("Bパート"));
    }

    #[test]
    fn chapter_at_a_time() {
        let chapters = vec![("A".to_string(), 0.0), ("B".to_string(), 100.0)];
        assert_eq!(chapter_at(&chapters, 50.0).map(|c| c.0.as_str()), Some("A"));
        assert_eq!(
            chapter_at(&chapters, 100.0).map(|c| c.0.as_str()),
            Some("B")
        );
        assert_eq!(chapter_at(&[], 5.0), None);
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
