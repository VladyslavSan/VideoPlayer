//! Intro, recap and credits timestamps from the community databases
//! Infuse uses too: TheIntroDB (theintrodb.org) and IntroDB (introdb.app).
//! Both are free and anonymous to read. They fill in what the Jellyfin
//! server doesn't have (it only has markers if a detection plugin ran).

use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;
use ureq::Agent;

use crate::video::{Segment, SegmentKind};

const THEINTRODB: &str = "https://api.theintrodb.org/v4/media";
const INTRODB: &str = "https://api.introdb.app/segments";
/// Asked while the video starts; skip buttons just don't appear if slow.
const TIMEOUT: Duration = Duration::from_secs(5);
const USER_AGENT: &str = concat!("VideoPlayer/", env!("CARGO_PKG_VERSION"));

/// What to look up: a movie, or an episode of a show. The ids are the
/// movie's, or the show's (not the episode's) for an episode.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Lookup {
    /// Season and episode, for an episode.
    pub episode: Option<(u16, u16)>,
    /// The runtime in seconds, which tells releases apart (TheIntroDB) and
    /// ends segments that run "to the end".
    pub duration: Option<f64>,
    pub tvdb: Option<String>,
    pub tmdb: Option<String>,
    pub imdb: Option<String>,
}

/// Segments for `lookup` from both databases, TheIntroDB first; IntroDB
/// fills in the kinds it lacks. Failures are logged, not returned: these
/// are extras.
pub fn fetch(lookup: &Lookup) -> Vec<Segment> {
    if lookup.tvdb.is_none() && lookup.tmdb.is_none() && lookup.imdb.is_none() {
        eprintln!("[segments] no TVDB/TMDB/IMDb id to look up");
        return Vec::new();
    }
    let agent: Agent = Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .user_agent(USER_AGENT)
        .build()
        .into();
    let first = theintrodb(&agent, lookup).unwrap_or_else(|e| {
        eprintln!("[segments] TheIntroDB: {e:#}");
        Vec::new()
    });
    let second = introdb(&agent, lookup).unwrap_or_else(|e| {
        eprintln!("[segments] IntroDB: {e:#}");
        Vec::new()
    });
    crate::player::merge_segments(&[&first, &second])
}

fn theintrodb(agent: &Agent, lookup: &Lookup) -> Result<Vec<Segment>> {
    // TVDB is TheIntroDB's own identity; TMDB and IMDb are fallbacks.
    let id = [
        ("tvdb_id", &lookup.tvdb),
        ("tmdb_id", &lookup.tmdb),
        ("imdb_id", &lookup.imdb),
    ]
    .into_iter()
    .find_map(|(key, value)| value.as_ref().map(|v| (key, v.clone())));
    let Some((key, value)) = id else {
        return Ok(Vec::new());
    };
    let mut request = agent.get(THEINTRODB).query(key, value);
    if let Some((season, episode)) = lookup.episode {
        request = request
            .query("season", season.to_string())
            .query("episode", episode.to_string());
    }
    if let Some(duration) = lookup.duration {
        request = request.query("duration_ms", ((duration * 1000.0) as i64).to_string());
    }
    let mut response = request.call().context("asking theintrodb.org")?;
    let media: TheIntroDbMedia = response.body_mut().read_json()?;
    Ok(media.segments(lookup.duration))
}

fn introdb(agent: &Agent, lookup: &Lookup) -> Result<Vec<Segment>> {
    let Some(imdb) = &lookup.imdb else {
        return Ok(Vec::new());
    };
    let mut request = agent.get(INTRODB).query("imdb_id", imdb);
    request = match lookup.episode {
        Some((season, episode)) => request
            .query("season", season.to_string())
            .query("episode", episode.to_string()),
        None => request.query("is_movie", "true"),
    };
    let mut response = request.call().context("asking introdb.app")?;
    let media: IntroDbMedia = response.body_mut().read_json()?;
    Ok(media.segments(lookup.duration))
}

/// `{"intro": [{"start_ms": 86231, "end_ms": 152172}], "credits": [...]}`.
/// A null start means from the beginning, a null end to the end.
#[derive(Debug, Default, Deserialize)]
struct TheIntroDbMedia {
    #[serde(default)]
    intro: Vec<TheIntroDbSpan>,
    #[serde(default)]
    recap: Vec<TheIntroDbSpan>,
    #[serde(default)]
    credits: Vec<TheIntroDbSpan>,
    #[serde(default)]
    preview: Vec<TheIntroDbSpan>,
}

#[derive(Debug, Deserialize)]
struct TheIntroDbSpan {
    start_ms: Option<f64>,
    end_ms: Option<f64>,
}

impl TheIntroDbMedia {
    fn segments(&self, duration: Option<f64>) -> Vec<Segment> {
        let kinds = [
            (SegmentKind::Recap, &self.recap),
            (SegmentKind::Intro, &self.intro),
            (SegmentKind::Credits, &self.credits),
            (SegmentKind::Preview, &self.preview),
        ];
        let segments = kinds.into_iter().flat_map(|(kind, spans)| {
            spans.iter().filter_map(move |s| {
                let start = s.start_ms.map_or(0.0, |ms| ms / 1000.0);
                let end = match s.end_ms {
                    Some(ms) => ms / 1000.0,
                    None => duration?,
                };
                Some(Segment { kind, start, end })
            })
        });
        valid(segments, duration)
    }
}

/// `{"intro": {"start_sec": 85, "end_sec": 152, ...}, "recap": null, ...}`.
#[derive(Debug, Default, Deserialize)]
struct IntroDbMedia {
    intro: Option<IntroDbSpan>,
    recap: Option<IntroDbSpan>,
    /// End credits. (`post_credits`, a scene after them, isn't skipped.)
    outro: Option<IntroDbSpan>,
}

#[derive(Debug, Deserialize)]
struct IntroDbSpan {
    start_sec: f64,
    end_sec: f64,
}

impl IntroDbMedia {
    fn segments(&self, duration: Option<f64>) -> Vec<Segment> {
        let kinds = [
            (SegmentKind::Recap, &self.recap),
            (SegmentKind::Intro, &self.intro),
            (SegmentKind::Credits, &self.outro),
        ];
        let segments = kinds.into_iter().filter_map(|(kind, span)| {
            span.as_ref().map(|s| Segment {
                kind,
                start: s.start_sec,
                end: s.end_sec,
            })
        });
        valid(segments, duration)
    }
}

/// Drops spans that can't belong to this file: empty, or past its end
/// (timed on a longer cut). A little slack covers rounding.
fn valid(segments: impl Iterator<Item = Segment>, duration: Option<f64>) -> Vec<Segment> {
    segments
        .filter(|s| s.end > s.start && s.start >= 0.0)
        .filter(|s| duration.is_none_or(|d| s.end <= d + 5.0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(kind: SegmentKind, start: f64, end: f64) -> Segment {
        Segment { kind, start, end }
    }

    #[test]
    fn theintrodb_response() {
        let media: TheIntroDbMedia = serde_json::from_str(
            r#"{"tvdb_id":161511,"type":"tv","season":3,"episode":2,
                "intro":[{"start_ms":86231,"end_ms":152172}],
                "recap":[{"start_ms":null,"end_ms":83187}],
                "credits":[{"start_ms":3283000,"end_ms":null}]}"#,
        )
        .unwrap();
        assert_eq!(
            media.segments(Some(3313.0)),
            vec![
                seg(SegmentKind::Recap, 0.0, 83.187),
                seg(SegmentKind::Intro, 86.231, 152.172),
                seg(SegmentKind::Credits, 3283.0, 3313.0),
            ]
        );
        // Without a runtime, credits that run to the end can't be placed.
        assert_eq!(media.segments(None).len(), 2);
    }

    #[test]
    fn introdb_response() {
        let media: IntroDbMedia = serde_json::from_str(
            r#"{"imdb_id":"tt1586680","media_type":"tv","season":3,"episode":2,
                "intro":{"start_sec":85,"end_sec":152,"start_ms":85000,"end_ms":152000,"confidence":1},
                "recap":null,
                "outro":{"start_sec":3283,"end_sec":3312},
                "post_credits":{"start_sec":3312,"end_sec":3400},
                "no_intro":false,"no_recap":true,"no_outro":false}"#,
        )
        .unwrap();
        assert_eq!(
            media.segments(Some(3313.0)),
            vec![
                seg(SegmentKind::Intro, 85.0, 152.0),
                seg(SegmentKind::Credits, 3283.0, 3312.0),
            ]
        );
    }

    #[test]
    fn spans_past_the_end_are_dropped() {
        let media: IntroDbMedia = serde_json::from_str(
            r#"{"intro":{"start_sec":85,"end_sec":152},"recap":null,
                "outro":{"start_sec":3500,"end_sec":3560}}"#,
        )
        .unwrap();
        // Credits timed on a longer cut than this 3313 s file.
        assert_eq!(
            media.segments(Some(3313.0)),
            vec![seg(SegmentKind::Intro, 85.0, 152.0)]
        );
    }

    /// Asks the real services about Shameless (US) S3E2:
    /// `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn live_lookup() {
        let segments = fetch(&Lookup {
            episode: Some((3, 2)),
            duration: Some(3313.0),
            tvdb: Some("161511".into()),
            tmdb: Some("34307".into()),
            imdb: Some("tt1586680".into()),
        });
        let kinds: Vec<_> = segments.iter().map(|s| s.kind).collect();
        assert!(kinds.contains(&SegmentKind::Intro), "{segments:?}");
        assert!(kinds.contains(&SegmentKind::Credits), "{segments:?}");
    }
}
