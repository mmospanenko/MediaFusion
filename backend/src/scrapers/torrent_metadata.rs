//! Shared `.torrent` download, parse, and privacy-type helpers for scrapers.

use std::time::Duration;

use lava_torrent::torrent::v1::Torrent;
use reqwest::Client;

use crate::db::TorrentType;

/// Maximum `.torrent` blob size stored in Postgres (2 MiB).
pub const MAX_TORRENT_FILE_BYTES: usize = 2 * 1024 * 1024;

/// A single file entry from a `.torrent` file list.
#[derive(Debug, Clone)]
pub struct TorrentFile {
    pub index: i32,
    pub path: String,
    pub size: i64,
}

/// Parsed metadata from a `.torrent` file.
#[derive(Debug, Clone)]
pub struct ParsedTorrent {
    pub info_hash: String,
    pub name: String,
    pub total_size: i64,
    pub announce_list: Vec<String>,
    pub raw_bytes: Vec<u8>,
    pub files: Vec<TorrentFile>,
}

/// Providers allowed to surface non-public torrent streams in the catalog.
pub const SUPPORTED_PRIVATE_TRACKER_PROVIDERS: &[&str] = &["debridlink", "qbittorrent", "torbox"];

pub fn is_probable_torrent_bytes(data: &[u8]) -> bool {
    data.first() == Some(&b'd')
}

pub fn parse_torrent_type_str(s: &str) -> TorrentType {
    match s.trim().to_lowercase().replace('-', "").as_str() {
        "semiprivate" => TorrentType::SemiPrivate,
        "private" => TorrentType::Private,
        "webseed" => TorrentType::WebSeed,
        _ => TorrentType::Public,
    }
}

/// Map Prowlarr indexer flags / privacy string to `TorrentType`.
pub fn prowlarr_torrent_type(indexer_flags: &[String], indexer_privacy: &str) -> TorrentType {
    for flag in indexer_flags {
        if flag.eq_ignore_ascii_case("semiPrivate") {
            return TorrentType::SemiPrivate;
        }
        if flag.eq_ignore_ascii_case("private") {
            return TorrentType::Private;
        }
    }
    if indexer_flags
        .iter()
        .any(|f| f.eq_ignore_ascii_case("freeleech"))
    {
        return TorrentType::Public;
    }
    if let Some(flag) = indexer_flags.first() {
        return parse_torrent_type_str(flag);
    }
    parse_torrent_type_str(indexer_privacy)
}

/// Map Jackett `TrackerType` field to `TorrentType`.
pub fn jackett_torrent_type(tracker_type: Option<&str>) -> TorrentType {
    parse_torrent_type_str(tracker_type.unwrap_or("public"))
}

pub fn is_private_torrent_type(t: TorrentType) -> bool {
    matches!(t, TorrentType::Private | TorrentType::SemiPrivate)
}

/// Whether raw `.torrent` bytes should be persisted for this type.
pub fn should_persist_torrent_file(t: TorrentType) -> bool {
    is_private_torrent_type(t)
}

/// Whether a torrent download is needed for this result.
/// Returns true when:
/// - The tracker type requires persisting the `.torrent` file (private/semi-private)
/// - No info_hash is available (need to extract from the `.torrent`)
/// - It's a series season pack (episodes empty, seasons non-empty) on a public tracker
pub fn needs_torrent_download(
    torrent_type: TorrentType,
    media_type: &str,
    parsed: &crate::parser::ParsedTitle,
    season: Option<i32>,
    info_hash: Option<&str>,
) -> bool {
    should_persist_torrent_file(torrent_type)
        || info_hash.is_none()
        || (media_type == "series"
            && parsed.episodes.is_empty()
            && !parsed.seasons.is_empty()
            && season.is_some())
}

pub fn torrent_file_for_storage(
    torrent_type: TorrentType,
    bytes: Option<Vec<u8>>,
) -> Option<Vec<u8>> {
    if !should_persist_torrent_file(torrent_type) {
        return None;
    }
    let bytes = bytes?;
    if bytes.is_empty() || bytes.len() > MAX_TORRENT_FILE_BYTES {
        return None;
    }
    Some(bytes)
}

/// Pick the URL to fetch for torrent metadata (Python `get_download_url` parity).
pub fn resolve_download_url(
    torrent_type: TorrentType,
    guid: Option<&str>,
    magnet_url: Option<&str>,
    download_url: Option<&str>,
) -> Option<String> {
    let download = download_url.filter(|u| !u.is_empty());
    let magnet = magnet_url.filter(|u| !u.is_empty());
    let guid = guid.filter(|u| !u.is_empty());

    if is_private_torrent_type(torrent_type)
        && let Some(d) = download
    {
        return Some(d.to_string());
    }

    if let Some(g) = guid.filter(|g| g.starts_with("magnet:")) {
        return Some(g.to_string());
    }

    if let Some(m) = magnet.filter(|m| m.starts_with("magnet:")) {
        return Some(m.to_string());
    }

    if let Some(d) = download
        && d.starts_with("magnet:")
    {
        return Some(d.to_string());
    }

    magnet
        .or(guid)
        .map(str::to_string)
        .or_else(|| download.map(str::to_string))
}

pub fn announce_list_from_magnet(magnet: &str) -> Vec<String> {
    magnet
        .split('&')
        .filter_map(|part| {
            let part = part.trim_start_matches('?');
            part.strip_prefix("tr=")
                .map(|v| urlencoding::decode(v).unwrap_or_default().into_owned())
        })
        .filter(|u| !u.is_empty())
        .collect()
}

pub async fn download_torrent_bytes(
    http: &Client,
    url: &str,
    timeout: Duration,
) -> Option<Vec<u8>> {
    if url.starts_with("magnet:") {
        return None;
    }
    let bytes = http
        .get(url)
        .timeout(timeout)
        .send()
        .await
        .ok()?
        .bytes()
        .await
        .ok()?
        .to_vec();
    if is_probable_torrent_bytes(&bytes) {
        Some(bytes)
    } else {
        None
    }
}

const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "avi", "webm", "mov", "m4v", "ts", "wmv", "flv", "vob", "ogv", "ogg", "mts",
    "m2ts", "iso",
];

pub fn is_video_name(path: &str) -> bool {
    let lower = path.to_lowercase();
    let ext = std::path::Path::new(&lower)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    VIDEO_EXTENSIONS.contains(&ext)
}

pub fn files_from_torrent(torrent: &Torrent) -> Vec<TorrentFile> {
    match &torrent.files {
        Some(files) => files
            .iter()
            .enumerate()
            .filter_map(|(i, f)| {
                let path = f.path.to_string_lossy().into_owned();
                if is_video_name(&path) {
                    Some(TorrentFile {
                        index: i as i32,
                        path,
                        size: f.length,
                    })
                } else {
                    None
                }
            })
            .collect(),
        None => {
            let path = torrent.name.clone();
            if is_video_name(&path) {
                vec![TorrentFile {
                    index: 0,
                    path,
                    size: torrent.length,
                }]
            } else {
                vec![]
            }
        }
    }
}

pub fn extract_file_list(bytes: &[u8]) -> Option<Vec<TorrentFile>> {
    let torrent = Torrent::read_from_bytes(bytes).ok()?;
    Some(files_from_torrent(&torrent))
}

pub fn parse_torrent_bytes(bytes: &[u8]) -> Option<ParsedTorrent> {
    if !is_probable_torrent_bytes(bytes) {
        return None;
    }
    let torrent = Torrent::read_from_bytes(bytes).ok()?;
    let info_hash = torrent.info_hash();
    if info_hash.len() != 40 || !info_hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }

    let mut announce_list = Vec::new();
    if let Some(announce) = &torrent.announce
        && !announce.is_empty()
    {
        announce_list.push(announce.clone());
    }
    if let Some(list) = &torrent.announce_list {
        for tier in list {
            for url in tier {
                if !url.is_empty() && !announce_list.contains(url) {
                    announce_list.push(url.clone());
                }
            }
        }
    }

    let files = files_from_torrent(&torrent);

    Some(ParsedTorrent {
        info_hash: info_hash.to_lowercase(),
        name: torrent.name.clone(),
        total_size: torrent.length,
        announce_list,
        raw_bytes: bytes.to_vec(),
        files,
    })
}

pub fn provider_supports_private_trackers(service: &str) -> bool {
    SUPPORTED_PRIVATE_TRACKER_PROVIDERS.contains(&service)
}

/// Whether a torrent row should be shown for the given provider (catalog filter parity).
pub fn private_torrent_visible_for_provider(
    torrent_type: TorrentType,
    provider_service: &str,
    has_torrent_providers: bool,
) -> bool {
    if !is_private_torrent_type(torrent_type) {
        return true;
    }
    if !has_torrent_providers {
        return false;
    }
    provider_supports_private_trackers(provider_service)
}

pub fn torrent_type_from_json_value(t: &serde_json::Value) -> TorrentType {
    t.get("torrent_type")
        .and_then(|v| v.as_str())
        .map(parse_torrent_type_str)
        .unwrap_or(TorrentType::Public)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_fixture(name: &str) -> Vec<u8> {
        let path = format!("tests/fixtures/{name}");
        std::fs::read(&path).expect("fixture not found")
    }

    // ── Seam 1: extract_file_list (adapter over lava_torrent) ──────────────

    #[test]
    fn extracts_video_files_from_multi_file_torrent() {
        let bytes = load_fixture("bojack_s01_bdrip_1080p.torrent");
        let files = extract_file_list(&bytes).expect("should parse");
        assert_eq!(files.len(), 12);
        for f in &files {
            assert!(
                is_video_name(&f.path),
                "file {} is not video: {}",
                f.index,
                f.path
            );
        }
        assert!(!files.iter().any(|f| f.path.contains("cover.jpg")));
    }

    #[test]
    fn extracts_video_files_from_single_file_torrent() {
        let bytes = load_fixture("bojack_s01e1_5_webrip_720p.torrent");
        let files = extract_file_list(&bytes).expect("should parse");
        assert_eq!(files.len(), 5);
        for f in &files {
            assert!(is_video_name(&f.path));
        }
    }

    #[test]
    fn returns_none_for_invalid_bytes() {
        assert!(extract_file_list(b"").is_none());
        assert!(extract_file_list(b"not bencode").is_none());
    }

    // ── Seam 2: is_video_name (pure predicate) ──────────────────────────────

    #[test]
    fn detects_video_extensions() {
        assert!(is_video_name("test.mkv"));
        assert!(is_video_name("test.MP4"));
        assert!(is_video_name("test.AVI"));
    }

    #[test]
    fn rejects_non_video_extensions() {
        assert!(!is_video_name("cover.jpg"));
        assert!(!is_video_name("subtitles.srt"));
        assert!(!is_video_name("readme.txt"));
    }

    // ── Seam 4: needs_torrent_download (pure boolean logic) ────────────────

    #[test]
    fn true_for_public_series_season_pack() {
        let parsed = crate::parser::parse_title("Show.S01.BDRip");
        assert!(needs_torrent_download(
            TorrentType::Public,
            "series",
            &parsed,
            Some(1),
            Some("ad47e255cf017864ec2f5fee57bef18c4b309808"),
        ));
    }

    #[test]
    fn false_for_public_single_episode() {
        let parsed = crate::parser::parse_title("Show.S01E05.1080p");
        assert!(!needs_torrent_download(
            TorrentType::Public,
            "series",
            &parsed,
            Some(1),
            Some("ad47e255cf017864ec2f5fee57bef18c4b309808"),
        ));
    }

    #[test]
    fn true_for_private_any_media() {
        let parsed = crate::parser::parse_title("Show.S01E05.1080p");
        assert!(needs_torrent_download(
            TorrentType::Private,
            "series",
            &parsed,
            Some(1),
            Some("ad47e255cf017864ec2f5fee57bef18c4b309808"),
        ));
    }

    #[test]
    fn true_when_info_hash_missing() {
        let parsed = crate::parser::parse_title("Show.S01E05.1080p");
        assert!(needs_torrent_download(
            TorrentType::Public,
            "series",
            &parsed,
            Some(1),
            None,
        ));
    }

    #[test]
    fn false_for_movie_season_pack() {
        let parsed = crate::parser::parse_title("Show.S01.BDRip");
        assert!(!needs_torrent_download(
            TorrentType::Public,
            "movie",
            &parsed,
            Some(1),
            Some("ad47e255cf017864ec2f5fee57bef18c4b309808"),
        ));
    }

    #[test]
    fn false_for_series_with_episodes() {
        let parsed = crate::parser::parse_title("Show.S01E05.1080p");
        assert!(!needs_torrent_download(
            TorrentType::Public,
            "series",
            &parsed,
            Some(1),
            Some("ad47e255cf017864ec2f5fee57bef18c4b309808"),
        ));
    }

    // ── Existing tests (pure functions, no changes) ─────────────────────────

    #[test]
    fn resolve_download_url_prefers_download_for_private() {
        let url = resolve_download_url(
            TorrentType::Private,
            Some("magnet:?xt=urn:btih:abc"),
            Some("magnet:?xt=urn:btih:abc"),
            Some("https://indexer/torrent/1"),
        );
        assert_eq!(url.as_deref(), Some("https://indexer/torrent/1"));
    }

    #[test]
    fn resolve_download_url_prefers_magnet_for_public() {
        let url = resolve_download_url(
            TorrentType::Public,
            Some("magnet:?xt=urn:btih:deadbeef"),
            Some("magnet:?xt=urn:btih:deadbeef"),
            Some("https://indexer/torrent/1"),
        );
        assert_eq!(url.as_deref(), Some("magnet:?xt=urn:btih:deadbeef"));
    }

    #[test]
    fn should_persist_file_only_for_private_types() {
        assert!(!should_persist_torrent_file(TorrentType::Public));
        assert!(should_persist_torrent_file(TorrentType::Private));
        assert!(should_persist_torrent_file(TorrentType::SemiPrivate));
    }

    #[test]
    fn torrent_file_for_storage_rejects_oversized() {
        let huge = vec![0u8; MAX_TORRENT_FILE_BYTES + 1];
        assert!(torrent_file_for_storage(TorrentType::Private, Some(huge)).is_none());
        let ok = vec![0u8; 64];
        assert_eq!(
            torrent_file_for_storage(TorrentType::Private, Some(ok.clone())),
            Some(ok)
        );
        assert!(torrent_file_for_storage(TorrentType::Public, Some(vec![1, 2, 3])).is_none());
    }

    #[test]
    fn private_torrent_catalog_filter() {
        assert!(private_torrent_visible_for_provider(
            TorrentType::Public,
            "realdebrid",
            true
        ));
        assert!(!private_torrent_visible_for_provider(
            TorrentType::Private,
            "realdebrid",
            true
        ));
        assert!(private_torrent_visible_for_provider(
            TorrentType::Private,
            "torbox",
            true
        ));
        assert!(!private_torrent_visible_for_provider(
            TorrentType::Private,
            "torbox",
            false
        ));
    }

    #[test]
    fn resolve_download_url_private_falls_back_to_magnet() {
        let url = resolve_download_url(
            TorrentType::Private,
            Some("magnet:?xt=urn:btih:deadbeef"),
            Some("magnet:?xt=urn:btih:deadbeef"),
            None,
        );
        assert_eq!(url.as_deref(), Some("magnet:?xt=urn:btih:deadbeef"));
    }

    #[test]
    fn prowlarr_torrent_type_prefers_private_over_freeleech() {
        assert_eq!(
            prowlarr_torrent_type(&["freeleech".into(), "private".into()], "public"),
            TorrentType::Private
        );
    }

    #[test]
    fn parse_torrent_type_str_maps_variants() {
        assert_eq!(
            parse_torrent_type_str("semiPrivate"),
            TorrentType::SemiPrivate
        );
        assert_eq!(parse_torrent_type_str("private"), TorrentType::Private);
        assert_eq!(parse_torrent_type_str("webseed"), TorrentType::WebSeed);
        assert_eq!(parse_torrent_type_str("public"), TorrentType::Public);
    }
}
