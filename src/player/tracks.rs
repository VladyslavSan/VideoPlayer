//! Human-readable names for audio and subtitle tracks, and picking tracks
//! by remembered language.

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
}
