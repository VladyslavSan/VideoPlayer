//! Human-readable names for audio and subtitle tracks, and picking tracks
//! by remembered language.

use crate::library::SubtitlePref;
use crate::video::{Track, TrackKind};

/// Whether two language codes mean the same language: files mix ISO 639-1
/// and 639-2 (`en`, `eng`), and 639-2 has two forms for some (`ger`, `deu`).
pub fn same_language(a: &str, b: &str) -> bool {
    match (language_name(a), language_name(b)) {
        (Some(x), Some(y)) => x == y,
        _ => a.eq_ignore_ascii_case(b),
    }
}

/// The audio track to use for a preferred language.
pub fn pick_audio(tracks: &[Track], lang: &str) -> Option<i64> {
    tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Audio)
        .find(|t| t.lang.as_deref().is_some_and(|l| same_language(l, lang)))
        .map(|t| t.id)
}

/// The subtitle track to use for a preferred language: full subtitles
/// rather than a forced-only track when there is a choice.
pub fn pick_subtitle(tracks: &[Track], lang: &str) -> Option<i64> {
    let mut matching: Vec<&Track> = tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Subtitle)
        .filter(|t| t.lang.as_deref().is_some_and(|l| same_language(l, lang)))
        .collect();
    matching.sort_by_key(|t| t.forced);
    matching.first().map(|t| t.id)
}

/// The audio track to switch to for a remembered language: None when the
/// file lacks it, or it's the one playing already.
pub fn audio_to_select(tracks: &[Track], wanted: Option<&str>) -> Option<i64> {
    let id = pick_audio(tracks, wanted?)?;
    let playing = tracks.iter().any(|t| t.id == id && t.selected);
    (!playing).then_some(id)
}

/// What to do about subtitles for a remembered choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtitlePlan {
    /// Nothing is remembered: leave them as the file has them.
    Leave,
    Off,
    Select(i64),
    /// The language isn't there yet; it may still arrive as a subtitle file.
    Wait,
}

pub fn subtitle_plan(tracks: &[Track], wanted: Option<&SubtitlePref>) -> SubtitlePlan {
    match wanted {
        None => SubtitlePlan::Leave,
        Some(SubtitlePref::Off) => SubtitlePlan::Off,
        Some(SubtitlePref::Language(lang)) => {
            pick_subtitle(tracks, lang).map_or(SubtitlePlan::Wait, SubtitlePlan::Select)
        }
    }
}

/// A pick among the tracks of one kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Track(i64),
    /// No subtitles.
    Off,
}

/// The choice after the selected one of `kind`, going round; subtitles
/// include "off". None when there's nothing to choose from.
pub fn next_choice(tracks: &[Track], kind: TrackKind) -> Option<Choice> {
    let of_kind = || tracks.iter().filter(|t| t.kind == kind);
    let mut choices: Vec<Choice> = of_kind().map(|t| Choice::Track(t.id)).collect();
    if kind == TrackKind::Subtitle {
        choices.push(Choice::Off);
    }
    let current = of_kind()
        .find(|t| t.selected)
        .map_or(Choice::Off, |t| Choice::Track(t.id));
    let next = choices
        .iter()
        .position(|choice| *choice == current)
        .map_or(0, |i| (i + 1) % choices.len());
    choices.get(next).copied()
}

/// What `choice` among the tracks of `kind` is called in a note.
pub fn choice_name(tracks: &[Track], kind: TrackKind, choice: Choice) -> String {
    let Choice::Track(id) = choice else {
        return "Off".into();
    };
    tracks
        .iter()
        .filter(|t| t.kind == kind)
        .enumerate()
        .find(|(_, t)| t.id == id)
        .map_or_else(|| "Off".into(), |(i, track)| label(track, i + 1).primary)
}

/// Two lines for a track menu row: `English` / `SDH · SRT · External`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackLabel {
    pub primary: String,
    pub secondary: String,
}

/// `number` is the track's 1-based position among tracks of its kind.
pub fn label(track: &Track, number: usize) -> TrackLabel {
    let language = track.lang.as_deref().and_then(language_name);
    let title = track.title.as_deref().map(str::trim).filter(|t| {
        // Titles often just repeat the language ("English", "eng"), and
        // external files without one get their file name ("Stream.srt").
        !t.is_empty()
            && !language.is_some_and(|l| l.eq_ignore_ascii_case(t))
            && !track
                .lang
                .as_deref()
                .is_some_and(|l| l.eq_ignore_ascii_case(t))
            && !is_subtitle_file_name(t)
    });
    let primary = match (language, title) {
        (Some(language), _) => language.to_string(),
        (None, Some(title)) => title.to_string(),
        (None, None) => match track.lang.as_deref() {
            Some(code) if code != "und" => code.to_uppercase(),
            _ => format!("Track {number}"),
        },
    };

    let mut details: Vec<String> = Vec::new();
    if language.is_some()
        && let Some(title) = title
    {
        details.push(title.to_string());
    }
    if let Some(codec) = &track.codec {
        details.push(codec_name(codec));
    }
    if track.kind == TrackKind::Audio
        && let Some(channels) = track.channels
    {
        details.push(channels_name(channels));
    }
    if track.forced {
        details.push("Forced".into());
    }
    if track.external {
        details.push("External".into());
    }
    TrackLabel {
        primary,
        secondary: details.join(" · "),
    }
}

fn is_subtitle_file_name(title: &str) -> bool {
    title.rsplit_once('.').is_some_and(|(_, ext)| {
        matches!(
            ext.to_ascii_lowercase().as_str(),
            "srt" | "ass" | "ssa" | "vtt" | "sub" | "sup" | "idx"
        )
    })
}

pub fn codec_name(codec: &str) -> String {
    match codec {
        "subrip" | "srt" => "SRT",
        "ass" | "ssa" => "ASS",
        "webvtt" => "WebVTT",
        "mov_text" => "MP4 text",
        "hdmv_pgs_subtitle" => "PGS",
        "dvd_subtitle" => "VobSub",
        "dvb_subtitle" => "DVB",
        "ac3" => "Dolby Digital",
        "eac3" => "Dolby Digital+",
        "truehd" => "TrueHD",
        "dts" => "DTS",
        "aac" => "AAC",
        "mp3" => "MP3",
        "flac" => "FLAC",
        "opus" => "Opus",
        "vorbis" => "Vorbis",
        other if other.starts_with("pcm") => "PCM",
        other => return other.to_uppercase(),
    }
    .to_string()
}

pub fn channels_name(channels: i64) -> String {
    match channels {
        1 => "Mono".into(),
        2 => "Stereo".into(),
        6 => "5.1".into(),
        8 => "7.1".into(),
        n => format!("{n} ch"),
    }
}

/// English name for an ISO 639-1 or 639-2 code (both forms appear in files).
pub fn language_name(code: &str) -> Option<&'static str> {
    let code = code.to_ascii_lowercase();
    Some(match code.as_str() {
        "en" | "eng" => "English",
        "uk" | "ukr" => "Ukrainian",
        "ru" | "rus" => "Russian",
        "de" | "ger" | "deu" => "German",
        "fr" | "fre" | "fra" => "French",
        "es" | "spa" => "Spanish",
        "it" | "ita" => "Italian",
        "pt" | "por" => "Portuguese",
        "pl" | "pol" => "Polish",
        "cs" | "cze" | "ces" => "Czech",
        "sk" | "slo" | "slk" => "Slovak",
        "nl" | "dut" | "nld" => "Dutch",
        "sv" | "swe" => "Swedish",
        "no" | "nor" | "nb" | "nob" => "Norwegian",
        "da" | "dan" => "Danish",
        "fi" | "fin" => "Finnish",
        "hu" | "hun" => "Hungarian",
        "ro" | "rum" | "ron" => "Romanian",
        "bg" | "bul" => "Bulgarian",
        "el" | "gre" | "ell" => "Greek",
        "tr" | "tur" => "Turkish",
        "he" | "heb" => "Hebrew",
        "ar" | "ara" => "Arabic",
        "hi" | "hin" => "Hindi",
        "ja" | "jpn" => "Japanese",
        "ko" | "kor" => "Korean",
        "zh" | "chi" | "zho" => "Chinese",
        "th" | "tha" => "Thai",
        "vi" | "vie" => "Vietnamese",
        "id" | "ind" => "Indonesian",
        "hr" | "hrv" => "Croatian",
        "sr" | "srp" => "Serbian",
        "lt" | "lit" => "Lithuanian",
        "lv" | "lav" => "Latvian",
        "et" | "est" => "Estonian",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(kind: TrackKind) -> Track {
        Track {
            kind,
            id: 1,
            title: None,
            lang: None,
            codec: None,
            channels: None,
            selected: false,
            external: false,
            forced: false,
        }
    }

    fn with(kind: TrackKind, id: i64, lang: &str, forced: bool) -> Track {
        Track {
            id,
            lang: Some(lang.into()),
            forced,
            ..track(kind)
        }
    }

    #[test]
    fn languages_match_across_code_styles() {
        assert!(same_language("en", "eng"));
        assert!(same_language("ger", "deu"));
        assert!(same_language("UKR", "uk"));
        assert!(!same_language("en", "uk"));
        assert!(same_language("tlh", "TLH"));
    }

    #[test]
    fn picks_tracks_by_language() {
        let tracks = vec![
            with(TrackKind::Audio, 1, "eng", false),
            with(TrackKind::Audio, 2, "ukr", false),
            with(TrackKind::Subtitle, 3, "uk", true),
            with(TrackKind::Subtitle, 4, "ukr", false),
            with(TrackKind::Subtitle, 5, "eng", false),
        ];
        assert_eq!(pick_audio(&tracks, "uk"), Some(2));
        assert_eq!(pick_audio(&tracks, "fr"), None);
        // Full Ukrainian subtitles beat the forced-only track.
        assert_eq!(pick_subtitle(&tracks, "ukr"), Some(4));
        assert_eq!(pick_subtitle(&tracks, "en"), Some(5));
        assert_eq!(pick_subtitle(&tracks, "de"), None);
    }

    #[test]
    fn audio_with_language_codec_and_channels() {
        let t = Track {
            lang: Some("eng".into()),
            codec: Some("eac3".into()),
            channels: Some(6),
            ..track(TrackKind::Audio)
        };
        let l = label(&t, 1);
        assert_eq!(l.primary, "English");
        assert_eq!(l.secondary, "Dolby Digital+ · 5.1");
    }

    #[test]
    fn title_repeating_the_language_is_dropped() {
        let t = Track {
            lang: Some("ukr".into()),
            title: Some("Ukrainian".into()),
            codec: Some("subrip".into()),
            external: true,
            ..track(TrackKind::Subtitle)
        };
        assert_eq!(
            label(&t, 1),
            TrackLabel {
                primary: "Ukrainian".into(),
                secondary: "SRT · External".into()
            }
        );
    }

    #[test]
    fn file_name_title_is_dropped() {
        let t = Track {
            lang: Some("eng".into()),
            title: Some("Stream.srt".into()),
            codec: Some("subrip".into()),
            external: true,
            ..track(TrackKind::Subtitle)
        };
        assert_eq!(label(&t, 1).secondary, "SRT · External");
    }

    #[test]
    fn descriptive_title_is_kept() {
        let t = Track {
            lang: Some("en".into()),
            title: Some("SDH".into()),
            codec: Some("hdmv_pgs_subtitle".into()),
            forced: true,
            ..track(TrackKind::Subtitle)
        };
        assert_eq!(label(&t, 1).secondary, "SDH · PGS · Forced");
    }

    #[test]
    fn fallbacks() {
        let titled = Track {
            title: Some("Director's commentary".into()),
            ..track(TrackKind::Audio)
        };
        assert_eq!(label(&titled, 2).primary, "Director's commentary");
        let unknown_code = Track {
            lang: Some("tlh".into()),
            ..track(TrackKind::Audio)
        };
        assert_eq!(label(&unknown_code, 2).primary, "TLH");
        let undetermined = Track {
            lang: Some("und".into()),
            ..track(TrackKind::Audio)
        };
        assert_eq!(label(&undetermined, 3).primary, "Track 3");
    }

    #[test]
    fn a_remembered_audio_language_is_selected_unless_it_plays_already() {
        let mut tracks = vec![
            with(TrackKind::Audio, 1, "jpn", false),
            with(TrackKind::Audio, 2, "eng", false),
        ];
        tracks[0].selected = true;
        assert_eq!(audio_to_select(&tracks, Some("en")), Some(2));
        assert_eq!(audio_to_select(&tracks, Some("ja")), None);
        assert_eq!(audio_to_select(&tracks, Some("fr")), None);
        assert_eq!(audio_to_select(&tracks, None), None);
    }

    #[test]
    fn remembered_subtitles_wait_for_a_language_that_is_not_there_yet() {
        let tracks = vec![with(TrackKind::Subtitle, 3, "eng", false)];
        let language = |code: &str| SubtitlePref::Language(code.into());
        assert_eq!(subtitle_plan(&tracks, None), SubtitlePlan::Leave);
        assert_eq!(
            subtitle_plan(&tracks, Some(&SubtitlePref::Off)),
            SubtitlePlan::Off
        );
        assert_eq!(
            subtitle_plan(&tracks, Some(&language("en"))),
            SubtitlePlan::Select(3)
        );
        assert_eq!(
            subtitle_plan(&tracks, Some(&language("uk"))),
            SubtitlePlan::Wait
        );
    }

    #[test]
    fn stepping_through_subtitles_passes_off() {
        let mut tracks = vec![
            with(TrackKind::Audio, 1, "eng", false),
            with(TrackKind::Subtitle, 1, "eng", false),
            with(TrackKind::Subtitle, 2, "ger", false),
        ];
        // None selected is "off", the last choice: the first track follows.
        assert_eq!(
            next_choice(&tracks, TrackKind::Subtitle),
            Some(Choice::Track(1))
        );
        tracks[2].selected = true;
        assert_eq!(next_choice(&tracks, TrackKind::Subtitle), Some(Choice::Off));
        assert_eq!(next_choice(&[], TrackKind::Subtitle), Some(Choice::Off));
    }

    #[test]
    fn stepping_through_audio_goes_round() {
        let mut tracks = vec![
            with(TrackKind::Audio, 1, "eng", false),
            with(TrackKind::Audio, 2, "jpn", false),
        ];
        tracks[1].selected = true;
        assert_eq!(
            next_choice(&tracks, TrackKind::Audio),
            Some(Choice::Track(1))
        );
        assert_eq!(next_choice(&[], TrackKind::Audio), None);
    }

    #[test]
    fn a_choice_is_named_like_its_row_in_the_panel() {
        let tracks = vec![with(TrackKind::Subtitle, 4, "eng", false)];
        assert_eq!(
            choice_name(&tracks, TrackKind::Subtitle, Choice::Track(4)),
            "English"
        );
        assert_eq!(
            choice_name(&tracks, TrackKind::Subtitle, Choice::Off),
            "Off"
        );
    }
}
