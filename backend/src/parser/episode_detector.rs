use std::sync::OnceLock;

use regex::Regex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedEpisode {
    pub season: i32,
    pub episode: i32,
}

fn re_sxxexx() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)[sS](\d{1,2})[eE](\d{1,2})").unwrap())
}

fn re_1x04() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\b(\d{1,2})[xX](\d{2})\b").unwrap())
}

fn re_season_episode_text() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?i)[sS]eason\s+(\d{1,2})[^0-9]{1,20}[eE]pisode\s+(\d{1,2})").unwrap()
    })
}

fn re_bracketed() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"[\[\(](?:[sS])?(\d{1,2})[.\s]?(?:[eExX])(\d{1,2})[\]\)]").unwrap()
    })
}

fn re_period_sep() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\b(\d{1,2})\.(\d{2})\b").unwrap())
}

fn re_episode_only_dash() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)[_\-][eE]p?(\d{1,3})\b").unwrap())
}

fn re_ep_word() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)\bEp(?:isode)?[.\s_]?(\d{1,3})\b").unwrap())
}

fn re_absolute() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\b0*(\d{3})\b").unwrap())
}

fn re_bare_digits() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"[^a-zA-Z0-9](\d{1,2})([^a-zA-Z0-9]|$)").unwrap())
}

pub fn detect_episode(filename: &str, default_season: i32) -> Option<DetectedEpisode> {
    let base = std::path::Path::new(filename)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(filename);

    if let Some(cap) = re_sxxexx().captures(base) {
        let s: i32 = cap[1].parse().ok()?;
        let e: i32 = cap[2].parse().ok()?;
        return Some(DetectedEpisode { season: s, episode: e });
    }

    if let Some(cap) = re_1x04().captures(base) {
        let s: i32 = cap[1].parse().ok()?;
        let e: i32 = cap[2].parse().ok()?;
        return Some(DetectedEpisode { season: s, episode: e });
    }

    if let Some(cap) = re_season_episode_text().captures(base) {
        let s: i32 = cap[1].parse().ok()?;
        let e: i32 = cap[2].parse().ok()?;
        return Some(DetectedEpisode { season: s, episode: e });
    }

    if let Some(cap) = re_bracketed().captures(base) {
        let s: i32 = cap[1].parse().ok()?;
        let e: i32 = cap[2].parse().ok()?;
        return Some(DetectedEpisode { season: s, episode: e });
    }

    if let Some(cap) = re_period_sep().captures(base) {
        let s: i32 = cap[1].parse().ok()?;
        let e: i32 = cap[2].parse().ok()?;
        if s <= 30 && e <= 50 {
            return Some(DetectedEpisode { season: s, episode: e });
        }
    }

    if let Some(cap) = re_episode_only_dash().captures(base) {
        let e: i32 = cap[1].parse().ok()?;
        return Some(DetectedEpisode { season: default_season, episode: e });
    }

    if let Some(cap) = re_ep_word().captures(base) {
        let e: i32 = cap[1].parse().ok()?;
        return Some(DetectedEpisode { season: default_season, episode: e });
    }

    if let Some(cap) = re_absolute().captures(base) {
        let m = cap.get(0).unwrap();
        let before = &base[..m.start()];
        let after = &base[m.end()..];
        let hex_ctx = before.chars().rev().take(8).all(|c| c.is_ascii_hexdigit())
            || after.chars().take(8).all(|c| c.is_ascii_hexdigit());
        if !hex_ctx {
            let e: i32 = cap[1].parse().ok()?;
            if e > 0 && e <= 999 {
                return Some(DetectedEpisode { season: default_season, episode: e });
            }
        }
    }

    if let Some(cap) = re_bare_digits().captures(base) {
        let m = cap.get(0).unwrap();
        let e: i32 = cap[1].parse().ok()?;
        if (1..=50).contains(&e) {
            let full = m.as_str();
            if !(full.starts_with('.') && full.ends_with('.') && cap[1].len() == 1) {
                return Some(DetectedEpisode { season: default_season, episode: e });
            }
        }
    }

    None
}

const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "avi", "webm", "mov", "flv", "wmv", "m4v", "ts", "m2ts", "mpg", "mpeg",
];

pub fn is_video_file(filename: &str) -> bool {
    let lower = filename.to_lowercase();
    let ext = std::path::Path::new(&lower)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    VIDEO_EXTENSIONS.contains(&ext)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formula2_practice_detect_episode_does_not_steal() {
        let f = "01.Formula.2.2026.R07.British.Practice.SkyF1HD.1080P.mkv";
        assert_eq!(detect_episode(f, 1), None);
    }

    #[test]
    fn test_sxxexx() {
        let r = detect_episode("Show.S02E05.mkv", 1).unwrap();
        assert_eq!(r, DetectedEpisode { season: 2, episode: 5 });
    }

    #[test]
    fn test_1x04() {
        let r = detect_episode("Show.2x08.720p.mkv", 1).unwrap();
        assert_eq!(r, DetectedEpisode { season: 2, episode: 8 });
    }

    #[test]
    fn test_text_form() {
        let r = detect_episode("Season 3 Episode 7.mp4", 1).unwrap();
        assert_eq!(r, DetectedEpisode { season: 3, episode: 7 });
    }

    #[test]
    fn test_bracketed() {
        let r = detect_episode("[S01E03] Title.mkv", 1).unwrap();
        assert_eq!(r, DetectedEpisode { season: 1, episode: 3 });
    }

    #[test]
    fn test_ep_word() {
        let r = detect_episode("ShowName.Ep.07.mkv", 1).unwrap();
        assert_eq!(r, DetectedEpisode { season: 1, episode: 7 });
    }

    #[test]
    fn test_no_match_resolution() {
        assert!(detect_episode("Show.1080p.BluRay.mkv", 1).is_none());
    }

    #[test]
    fn test_bare_episode_underscore() {
        let r = detect_episode("[FanVoxUA]_Rick_and_Morty_02_[1080p].mkv", 1).unwrap();
        assert_eq!(r, DetectedEpisode { season: 1, episode: 2 });
    }

    #[test]
    fn test_bare_episode_dash() {
        let r = detect_episode("Show-02-720p.mkv", 1).unwrap();
        assert_eq!(r, DetectedEpisode { season: 1, episode: 2 });
    }

    #[test]
    fn test_bare_episode_dot() {
        let r = detect_episode("Show.02.720p.mkv", 1).unwrap();
        assert_eq!(r, DetectedEpisode { season: 1, episode: 2 });
    }

    #[test]
    fn test_bare_episode_space() {
        let r = detect_episode("Show 02 720p.mkv", 1).unwrap();
        assert_eq!(r, DetectedEpisode { season: 1, episode: 2 });
    }

    #[test]
    fn test_bare_episode_bracket() {
        let r = detect_episode("[02] Show.mkv", 1).unwrap();
        assert_eq!(r, DetectedEpisode { season: 1, episode: 2 });
    }

    #[test]
    fn test_bare_episode_dash_space() {
        let r = detect_episode("Show - 02.mkv", 1).unwrap();
        assert_eq!(r, DetectedEpisode { season: 1, episode: 2 });
    }

    #[test]
    fn test_bare_episode_single_digit() {
        let r = detect_episode("Show_1_1080p.mkv", 1).unwrap();
        assert_eq!(r, DetectedEpisode { season: 1, episode: 1 });
    }

    #[test]
    fn test_bare_episode_does_not_match_resolution() {
        assert!(detect_episode("Show.1080p.mkv", 1).is_none());
    }

    #[test]
    fn test_bare_episode_does_not_match_year() {
        assert!(detect_episode("Show.2024.1080p.mkv", 1).is_none());
    }

    #[test]
    fn test_bare_episode_does_not_match_racing() {
        assert!(detect_episode("01.Formula.2.2026.R07.British.Practice.SkyF1HD.1080P.mkv", 1).is_none());
    }

    #[test]
    fn test_is_video_file() {
        assert!(is_video_file("video.mkv"));
        assert!(is_video_file("video.MP4"));
        assert!(!is_video_file("readme.txt"));
    }
}
