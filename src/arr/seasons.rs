//! What a show's page says about Sonarr: per season, what has aired, what
//! is downloaded, what's missing and what's downloading.

use std::collections::BTreeMap;

use super::{Episode, QueueItem, Series};

#[derive(Debug, Clone, PartialEq)]
pub struct SeasonSummary {
    pub number: u32,
    /// Episodes that have aired.
    pub aired: usize,
    /// Of those, downloaded.
    pub downloaded: usize,
    /// Aired and not downloaded.
    pub missing: usize,
    /// Not aired yet (or no date).
    pub upcoming: usize,
    /// Every episode of the season, for monitoring them all.
    pub episode_ids: Vec<i64>,
    pub monitored: bool,
    /// Episodes in the download queue, and how far along they are (0..1).
    pub downloading: usize,
    pub progress: Option<f32>,
}

impl SeasonSummary {
    /// Worth a line on the show's page: something to get or being got.
    pub fn wanted(&self) -> bool {
        self.missing > 0 || self.downloading > 0
    }
}

/// `2020-12-28T02:00:00Z` (or with fractions of a second) → Unix seconds.
pub fn parse_utc(text: &str) -> Option<i64> {
    let (date, time) = text.trim().trim_end_matches('Z').split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (year, month, day) = (d.next()??, d.next()??, d.next()??);
    let mut t = time.split(':');
    let hour: i64 = t.next()?.parse().ok()?;
    let minute: i64 = t.next()?.parse().ok()?;
    let second: i64 = t
        .next()
        .and_then(|s| s.split('.').next())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second)
}

/// Unix seconds → `20 Nov 2026` (UTC).
pub fn format_date(at: i64) -> String {
    // Civil date from days (Howard Hinnant's algorithm).
    let z = at.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!("{day} {} {year}", MONTHS[(month - 1) as usize])
}

fn aired(episode: &Episode, now: i64) -> bool {
    episode
        .air_date_utc
        .as_deref()
        .and_then(parse_utc)
        .is_some_and(|at| at <= now)
}

/// Each numbered season (specials left out), in order.
pub fn summarize(
    series: &Series,
    episodes: &[Episode],
    queue: &[QueueItem],
    now: i64,
) -> Vec<SeasonSummary> {
    let mut seasons: BTreeMap<u32, SeasonSummary> = BTreeMap::new();
    for episode in episodes.iter().filter(|e| e.season_number > 0) {
        let season = seasons
            .entry(episode.season_number)
            .or_insert_with(|| SeasonSummary {
                number: episode.season_number,
                aired: 0,
                downloaded: 0,
                missing: 0,
                upcoming: 0,
                episode_ids: Vec::new(),
                monitored: series
                    .seasons
                    .iter()
                    .any(|s| s.season_number == episode.season_number && s.monitored),
                downloading: 0,
                progress: None,
            });
        season.episode_ids.push(episode.id);
        if episode.has_file {
            season.downloaded += 1;
            season.aired += usize::from(aired(episode, now));
        } else if aired(episode, now) {
            season.aired += 1;
            season.missing += 1;
        } else {
            season.upcoming += 1;
        }
    }
    for season in seasons.values_mut() {
        let (mut size, mut left) = (0.0, 0.0);
        for item in queue.iter().filter(|q| {
            q.episode_id
                .is_some_and(|id| season.episode_ids.contains(&id))
                && q.status.as_deref() != Some("completed")
        }) {
            season.downloading += 1;
            size += item.size;
            left += item.sizeleft;
        }
        if size > 0.0 {
            season.progress = Some((1.0 - left / size).clamp(0.0, 1.0) as f32);
        }
    }
    seasons.into_values().collect()
}

/// The next episode to air, if one has a date.
pub fn next_airing(episodes: &[Episode], now: i64) -> Option<(&Episode, i64)> {
    episodes
        .iter()
        .filter(|e| e.season_number > 0)
        .filter_map(|e| Some((e, e.air_date_utc.as_deref().and_then(parse_utc)?)))
        .filter(|(_, at)| *at > now)
        .min_by_key(|(_, at)| *at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arr::SeriesSeason;

    fn episode(id: i64, season: u32, number: u32, aired: Option<&str>, file: bool) -> Episode {
        Episode {
            id,
            season_number: season,
            episode_number: number,
            title: None,
            air_date_utc: aired.map(String::from),
            has_file: file,
        }
    }

    #[test]
    fn parses_sonarr_dates() {
        assert_eq!(parse_utc("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_utc("2020-12-28T02:00:00Z"), Some(1_609_120_800));
        assert_eq!(parse_utc("2024-02-29T12:30:15.5Z"), Some(1_709_209_815));
        assert_eq!(parse_utc("soon"), None);
        assert_eq!(format_date(1_609_120_800), "28 Dec 2020");
        assert_eq!(format_date(1_709_209_815), "29 Feb 2024");
        assert_eq!(format_date(0), "1 Jan 1970");
    }

    #[test]
    fn seasons_count_missing_upcoming_and_downloading() {
        let now = parse_utc("2026-11-13T00:00:00Z").unwrap();
        let past = Some("2020-01-01T00:00:00Z");
        let future = Some("2027-01-01T00:00:00Z");
        let series = Series {
            id: 1,
            seasons: vec![SeriesSeason {
                season_number: 2,
                monitored: true,
            }],
        };
        let episodes = vec![
            episode(1, 0, 1, past, false), // a special: left out
            episode(2, 1, 1, past, true),
            episode(3, 1, 2, past, true),
            episode(4, 2, 1, past, false),
            episode(5, 2, 2, past, false),
            episode(6, 2, 3, future, false),
            episode(7, 2, 4, None, false),
        ];
        let queue = vec![
            QueueItem {
                episode_id: Some(4),
                size: 100.0,
                sizeleft: 25.0,
                status: Some("downloading".into()),
            },
            QueueItem {
                episode_id: Some(5),
                size: 100.0,
                sizeleft: 0.0,
                status: Some("completed".into()),
            },
        ];
        let seasons = summarize(&series, &episodes, &queue, now);
        assert_eq!(seasons.len(), 2);
        let (one, two) = (&seasons[0], &seasons[1]);
        assert_eq!(
            (one.number, one.aired, one.downloaded, one.missing),
            (1, 2, 2, 0)
        );
        assert!(!one.wanted() && !one.monitored);
        assert_eq!((two.aired, two.missing, two.upcoming), (2, 2, 2));
        assert_eq!(two.episode_ids, vec![4, 5, 6, 7]);
        assert!(two.monitored && two.wanted());
        assert_eq!((two.downloading, two.progress), (1, Some(0.75)));
        let (next, _) = next_airing(&episodes, now).unwrap();
        assert_eq!(next.id, 6);
    }
}
