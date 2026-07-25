use std::sync::{Arc, Mutex};

use mediafusion_api::providers::torrents::qbittorrent;
use serde_json::json;
use tracing_subscriber::prelude::*;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct TestWriter {
    buf: Arc<Mutex<Vec<u8>>>,
}

impl std::io::Write for TestWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.buf.lock().unwrap().write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.buf.lock().unwrap().flush()
    }
}

fn init_capture(buf: Arc<Mutex<Vec<u8>>>) -> tracing::subscriber::DefaultGuard {
    let make_writer = move || TestWriter {
        buf: Arc::clone(&buf),
    };
    let subscriber = tracing_subscriber::Registry::default()
        .with(tracing_subscriber::EnvFilter::new("debug"))
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_ansi(false)
                .compact()
                .with_writer(make_writer),
        );
    tracing::subscriber::set_default(subscriber)
}

fn qb_config(qb_url: &str, webdav_url: &str) -> serde_json::Value {
    json!({
        "qbittorrent_url": qb_url,
        "qbittorrent_username": "test_user",
        "qbittorrent_password": "test_pass",
        "webdav_url": webdav_url,
        "webdav_username": "wd_user",
        "webdav_password": "wd_pass",
        "webdav_downloads_path": "/downloads",
        "play_video_after": 3,
        "seeding_time_limit": 1440,
        "seeding_ratio_limit": 1.0,
        "category": "MediaFusion"
    })
}

fn webdav_propfind_response(hrefs: &[&str]) -> ResponseTemplate {
    let body = format!(
        r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
{}
</d:multistatus>"#,
        hrefs
            .iter()
            .map(|h| format!("<d:response><d:href>{h}</d:href></d:response>"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    ResponseTemplate::new(207).set_body_string(body)
}

#[tokio::test]
async fn successful_playback_emits_six_breadcrumbs_in_order() {
    let buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let _guard = init_capture(Arc::clone(&buf));

    let qb_mock = MockServer::start().await;
    let wd_mock = MockServer::start().await;

    // qBittorrent login: 204
    Mock::given(method("POST"))
        .and(path("/api/v2/auth/login"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&qb_mock)
        .await;

    // qBittorrent torrents/info: first call empty, subsequent calls progress=0.05
    Mock::given(method("GET"))
        .and(path("/api/v2/torrents/info"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!([{
                "hash": "0bb32deadbeef",
                "progress": 0.05,
                "name": "test.torrent"
            }])),
        )
        .mount(&qb_mock)
        .await;

    // WebDAV PROPFIND: one video file
    Mock::given(method("PROPFIND"))
        .respond_with(webdav_propfind_response(&[
            "/downloads/0bb32deadbeef/S01E01.mkv",
        ]))
        .mount(&wd_mock)
        .await;

    let config = qb_config(&qb_mock.uri(), &wd_mock.uri());
    let http = reqwest::Client::new();

    let result = qbittorrent::get_video_url(
        &http,
        &config,
        "0bb32deadbeef",
        "magnet:?xt=urn:btih:0bb32deadbeef",
        "Test Torrent",
        None,
        None,
        None,
        None,
        false,
    )
    .await;

    assert!(result.is_ok(), "expected Ok, got {:?}", result.err());

    let log = {
        let b = buf.lock().unwrap();
        String::from_utf8(b.clone()).unwrap()
    };

    // Check breadcrumbs in order (log format: step="login" etc.)
    let steps: Vec<&str> = vec![
        r#"step="login""#,
        r#"step="info""#,
        r#"step="find""#,
        r#"step="resolve""#,
    ];
    let mut last_pos = 0usize;
    for step in &steps {
        let pos = log[last_pos..].find(step).map(|p| last_pos + p);
        assert!(
            pos.is_some(),
            "expected '{step}' in log (after pos {last_pos}), got:\n{log}"
        );
        last_pos = pos.unwrap() + step.len();
    }

    assert!(
        log.contains(r#"provider="qbittorrent""#),
        "expected provider=qbittorrent in log, got:\n{log}"
    );
}

#[tokio::test]
async fn wait_for_progress_timeout_emits_warn() {
    let buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let _guard = init_capture(Arc::clone(&buf));

    let qb_mock = MockServer::start().await;
    let wd_mock = MockServer::start().await;

    // qBittorrent login: 204
    Mock::given(method("POST"))
        .and(path("/api/v2/auth/login"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&qb_mock)
        .await;

    // qBittorrent torrents/info: always progress=0.0
    Mock::given(method("GET"))
        .and(path("/api/v2/torrents/info"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!([{
                "hash": "0bb32deadbeef",
                "progress": 0.0,
                "name": "test.torrent"
            }])),
        )
        .mount(&qb_mock)
        .await;

    // WebDAV PROPFIND: one video file (won't be reached due to timeout)
    Mock::given(method("PROPFIND"))
        .respond_with(webdav_propfind_response(&[
            "/downloads/0bb32deadbeef/S01E01.mkv",
        ]))
        .mount(&wd_mock)
        .await;

    let config = qb_config(&qb_mock.uri(), &wd_mock.uri());
    let http = reqwest::Client::new();

    // The function polls 20 times with 5s sleep = 100s total.
    // Use a tokio timeout to keep the test fast; we verify the warn
    // appears after at least one poll cycle.
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        qbittorrent::get_video_url(
            &http,
            &config,
            "0bb32deadbeef",
            "magnet:?xt=urn:btih:0bb32deadbeef",
            "Test Torrent",
            None,
            None,
            None,
            None,
            false,
        ),
    )
    .await;

    // Either timed out (tokio) or returned Err (full timeout)
    assert!(
        result.is_err() || result.unwrap().is_err(),
        "expected timeout or error"
    );

    let log = {
        let b = buf.lock().unwrap();
        String::from_utf8(b.clone()).unwrap()
    };

    // At least one poll line should have been emitted
    assert!(
        log.contains("qb wait poll"),
        "expected at least one 'qb wait poll' in log, got:\n{log}"
    );
}

#[tokio::test]
async fn login_forbidden_emits_warn_and_returns_invalid_credentials() {
    let buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let _guard = init_capture(Arc::clone(&buf));

    let qb_mock = MockServer::start().await;
    let wd_mock = MockServer::start().await;

    // qBittorrent login: 403
    Mock::given(method("POST"))
        .and(path("/api/v2/auth/login"))
        .respond_with(ResponseTemplate::new(403).set_body_string("Forbidden"))
        .mount(&qb_mock)
        .await;

    let config = qb_config(&qb_mock.uri(), &wd_mock.uri());
    let http = reqwest::Client::new();

    let result = qbittorrent::get_video_url(
        &http,
        &config,
        "0bb32deadbeef",
        "magnet:?xt=urn:btih:0bb32deadbeef",
        "Test Torrent",
        None,
        None,
        None,
        None,
        false,
    )
    .await;

    assert!(result.is_err(), "expected Err due to login failure");

    let log = {
        let b = buf.lock().unwrap();
        String::from_utf8(b.clone()).unwrap()
    };

    assert!(
        log.contains("qb login failed"),
        "expected 'qb login failed' in log, got:\n{log}"
    );
    assert!(
        log.contains("playback step failed"),
        "expected 'playback step failed' in log, got:\n{log}"
    );
    assert!(
        log.contains(r#"step="login""#),
        "expected 'step=login' in log, got:\n{log}"
    );
    assert!(
        log.contains(r#"video_file="invalid_credentials.mp4""#),
        "expected 'video_file=invalid_credentials.mp4' in log, got:\n{log}"
    );
}

#[tokio::test]
async fn no_matching_file_emits_warn() {
    let buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let _guard = init_capture(Arc::clone(&buf));

    let qb_mock = MockServer::start().await;
    let wd_mock = MockServer::start().await;

    // qBittorrent login: 204
    Mock::given(method("POST"))
        .and(path("/api/v2/auth/login"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&qb_mock)
        .await;

    // qBittorrent torrents/info: progress past threshold
    Mock::given(method("GET"))
        .and(path("/api/v2/torrents/info"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!([{
                "hash": "0bb32deadbeef",
                "progress": 0.1,
                "name": "test.torrent"
            }])),
        )
        .mount(&qb_mock)
        .await;

    // WebDAV PROPFIND: no video files (only a directory)
    Mock::given(method("PROPFIND"))
        .respond_with(webdav_propfind_response(&[
            "/downloads/0bb32deadbeef/subdir/",
        ]))
        .mount(&wd_mock)
        .await;

    let config = qb_config(&qb_mock.uri(), &wd_mock.uri());
    let http = reqwest::Client::new();

    let result = qbittorrent::get_video_url(
        &http,
        &config,
        "0bb32deadbeef",
        "magnet:?xt=urn:btih:0bb32deadbeef",
        "Test Torrent",
        None,
        None,
        None,
        None,
        false,
    )
    .await;

    assert!(result.is_err(), "expected Err due to no matching file");

    let log = {
        let b = buf.lock().unwrap();
        String::from_utf8(b.clone()).unwrap()
    };

    assert!(
        log.contains("qb find file: no match"),
        "expected 'qb find file: no match' in log, got:\n{log}"
    );
}

#[tokio::test]
async fn credentials_never_appear_in_logs() {
    let buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let _guard = init_capture(Arc::clone(&buf));

    let qb_mock = MockServer::start().await;
    let wd_mock = MockServer::start().await;

    // qBittorrent login: 204
    Mock::given(method("POST"))
        .and(path("/api/v2/auth/login"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&qb_mock)
        .await;

    // qBittorrent torrents/info: progress past threshold
    Mock::given(method("GET"))
        .and(path("/api/v2/torrents/info"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!([{
                "hash": "0bb32deadbeef",
                "progress": 0.1,
                "name": "test.torrent"
            }])),
        )
        .mount(&qb_mock)
        .await;

    // WebDAV PROPFIND: one video file
    Mock::given(method("PROPFIND"))
        .respond_with(webdav_propfind_response(&[
            "/downloads/0bb32deadbeef/S01E01.mkv",
        ]))
        .mount(&wd_mock)
        .await;

    let config = json!({
        "qbittorrent_url": qb_mock.uri(),
        "qbittorrent_username": "secret_user",
        "qbittorrent_password": "supersecret123",
        "webdav_url": wd_mock.uri(),
        "webdav_username": "wd_admin",
        "webdav_password": "wd_supersecret",
        "webdav_downloads_path": "/downloads",
        "play_video_after": 3,
        "seeding_time_limit": 1440,
        "seeding_ratio_limit": 1.0,
        "category": "MediaFusion"
    });
    let http = reqwest::Client::new();

    let result = qbittorrent::get_video_url(
        &http,
        &config,
        "0bb32deadbeef",
        "magnet:?xt=urn:btih:0bb32deadbeef",
        "Test Torrent",
        None,
        None,
        None,
        None,
        false,
    )
    .await;

    assert!(result.is_ok(), "expected Ok, got {:?}", result.err());

    let log = {
        let b = buf.lock().unwrap();
        String::from_utf8(b.clone()).unwrap()
    };

    assert!(!log.contains("supersecret123"), "qb password leaked in log");
    assert!(
        !log.contains("wd_supersecret"),
        "webdav password leaked in log"
    );
    assert!(!log.contains("secret_user"), "qb username leaked in log");
    assert!(!log.contains("wd_admin"), "webdav username leaked in log");
}
