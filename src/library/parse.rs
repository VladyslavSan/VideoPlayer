//! Turns video file paths into movie or episode metadata, using common
//! release naming conventions (`Title (Year)`, `Show.S01E02`, `Show - 1x02`).

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    Movie {
        title: String,
        year: Option<u16>,
    },
    Episode {
        show: String,
        season: u16,
        episode: u16,
        title: Option<String>,
    },
}

static EPISODE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?P<show>.*?)[\s._-]*\bS(?P<s>\d{1,2})[\s._-]?E(?P<e>\d{1,3})(?:[\s._-]?E\d{1,3})*(?P<rest>.*)$",
    )
    .unwrap()
});

static EPISODE_X: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?P<show>.*?)[\s._-]*\b(?P<s>\d{1,2})x(?P<e>\d{2,3})\b(?P<rest>.*)$").unwrap()
});

/// Greedy title, so the last year wins: `Blade Runner 2049 (2017)`.
static YEAR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?P<title>.+)[\s._(\[-]+(?P<year>(?:19|20)\d{2})(?:[\s._)\]-]|$)").unwrap()
});

/// Release tags; everything from the first one onwards is noise.
static JUNK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)[\s._(\[-]+(?:2160p|1080p|1080i|720p|576p|480p|4k|uhd|hdr10\+?|hdr|dolby[\s._-]?vision|dv|bluray|blu-ray|bdrip|brrip|web[\s._-]?dl|webrip|web|hdtv|dvdrip|remux|x26[45]|h\.?26[45]|hevc|avc|aac|dts|ac3|eac3|atmos|truehd|proper|repack|extended|unrated|remastered|imax)\b.*$",
    )
    .unwrap()
});

static SEASON_DIR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?:season|series|s)[\s._-]*\d{1,2}$|^specials$").unwrap());

pub fn parse_path(path: &Path) -> Parsed {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

    if let Some(caps) = EPISODE
        .captures(&stem)
        .or_else(|| EPISODE_X.captures(&stem))
    {
        let season = caps["s"].parse().unwrap_or(0);
        let episode = caps["e"].parse().unwrap_or(0);
        let mut show = strip_year(&clean(&caps["show"]));
        if show.is_empty() {
            show = show_from_folders(path).unwrap_or_else(|| "Unknown Show".into());
        }
        let title = Some(clean(&JUNK.replace(&caps["rest"], ""))).filter(|t| !t.is_empty());
        return Parsed::Episode {
            show,
            season,
            episode,
            title,
        };
    }

    let name = JUNK.replace(&stem, "");
    match YEAR.captures(&name) {
        Some(caps) => Parsed::Movie {
            title: clean(&caps["title"]),
            year: caps["year"].parse().ok(),
        },
        None => Parsed::Movie {
            title: clean(&name),
            year: None,
        },
    }
}

/// Scene names use dots or underscores for spaces; names that already
/// contain spaces keep their dots (`Mr. Robot`).
fn clean(raw: &str) -> String {
    let spaced = if raw.contains(' ') {
        raw.replace('_', " ")
    } else {
        raw.replace(['.', '_'], " ")
    };
    spaced
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(|c: char| c == '-' || c == ' ' || c == '.')
        .to_string()
}

fn strip_year(show: &str) -> String {
    match YEAR.captures(show) {
        Some(caps) => clean(&caps["title"]),
        None => show.to_string(),
    }
}

/// `Show/Season 1/S01E02.mkv` → `Show`.
fn show_from_folders(path: &Path) -> Option<String> {
    let mut dirs = path.ancestors().skip(1).filter_map(|p| p.file_name());
    let first = dirs.next()?.to_string_lossy().into_owned();
    let name = if SEASON_DIR.is_match(&first) {
        dirs.next()?.to_string_lossy().into_owned()
    } else {
        first
    };
    Some(strip_year(&clean(&name))).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn movie(title: &str, year: Option<u16>) -> Parsed {
        Parsed::Movie {
            title: title.into(),
            year,
        }
    }

    fn episode(show: &str, season: u16, episode: u16, title: Option<&str>) -> Parsed {
        Parsed::Episode {
            show: show.into(),
            season,
            episode,
            title: title.map(Into::into),
        }
    }

    fn parse(path: &str) -> Parsed {
        parse_path(Path::new(path))
    }

    #[test]
    fn scene_movie() {
        assert_eq!(
            parse("D:/Movies/The.Matrix.1999.1080p.BluRay.x264.mkv"),
            movie("The Matrix", Some(1999))
        );
    }

    #[test]
    fn year_inside_title_uses_last_year() {
        assert_eq!(
            parse("Blade Runner 2049 (2017).mkv"),
            movie("Blade Runner 2049", Some(2017))
        );
        assert_eq!(
            parse("2001.A.Space.Odyssey.1968.mkv"),
            movie("2001 A Space Odyssey", Some(1968))
        );
    }

    #[test]
    fn title_that_is_a_year() {
        assert_eq!(parse("1917.mkv"), movie("1917", None));
    }

    #[test]
    fn plain_movie_without_year() {
        assert_eq!(
            parse("Home Movies_Summer.mp4"),
            movie("Home Movies Summer", None)
        );
    }

    #[test]
    fn scene_episode() {
        assert_eq!(
            parse("Breaking.Bad.S02E03.720p.WEB-DL.mkv"),
            episode("Breaking Bad", 2, 3, None)
        );
    }

    #[test]
    fn episode_with_title_keeps_dots_in_spaced_names() {
        assert_eq!(
            parse("Mr. Robot - S01E01 - Hello Friend.mkv"),
            episode("Mr. Robot", 1, 1, Some("Hello Friend"))
        );
    }

    #[test]
    fn x_style_episode() {
        assert_eq!(
            parse("The Office (US) - 3x07 - Branch Wars.mkv"),
            episode("The Office (US)", 3, 7, Some("Branch Wars"))
        );
    }

    #[test]
    fn show_year_is_dropped() {
        assert_eq!(
            parse("Doctor.Who.2005.S01E01.Rose.mkv"),
            episode("Doctor Who", 1, 1, Some("Rose"))
        );
    }

    #[test]
    fn show_from_folder_structure() {
        assert_eq!(
            parse("D:/TV/Severance/Season 1/S01E04.mkv"),
            episode("Severance", 1, 4, None)
        );
        assert_eq!(
            parse("D:/TV/Severance (2022)/S01E04.mkv"),
            episode("Severance", 1, 4, None)
        );
    }

    #[test]
    fn multi_episode_file() {
        assert_eq!(
            parse("Friends.S01E01E02.mkv"),
            episode("Friends", 1, 1, None)
        );
    }
}
