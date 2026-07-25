use std::sync::{Arc, Mutex};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    routing::get,
};
use tower::ServiceExt;
use tracing_subscriber::prelude::*;

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

#[tokio::test(flavor = "current_thread")]
async fn incoming_request_id_appears_on_span_and_response() {
    let buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let _guard = init_capture(Arc::clone(&buf));

    async fn handler() -> &'static str {
        tracing::info!("test handler event");
        "ok"
    }

    let app = Router::new()
        .route("/test", get(handler))
        .layer(mediafusion_api::make_trace_layer!())
        .layer(axum::middleware::from_fn(
            mediafusion_api::request_id_middleware::request_id_middleware,
        ));

    let req = Request::builder()
        .uri("/test")
        .header("x-request-id", "abc123")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp_id = resp
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok());
    assert_eq!(resp_id, Some("abc123"));

    let log = {
        let b = buf.lock().unwrap();
        String::from_utf8(b.clone()).unwrap()
    };
    assert!(
        log.contains("request_id=abc123"),
        "expected request_id=abc123 in log, got: {log}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn generated_id_agrees_between_span_and_response() {
    let buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let _guard = init_capture(Arc::clone(&buf));

    async fn handler() -> &'static str {
        tracing::info!("test handler event");
        "ok"
    }

    let app = Router::new()
        .route("/test", get(handler))
        .layer(mediafusion_api::make_trace_layer!())
        .layer(axum::middleware::from_fn(
            mediafusion_api::request_id_middleware::request_id_middleware,
        ));

    let req = Request::builder().uri("/test").body(Body::empty()).unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp_id = resp
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .expect("response must have x-request-id");
    assert_eq!(resp_id.len(), 32, "expected 32-char hex UUID");

    let log = {
        let b = buf.lock().unwrap();
        String::from_utf8(b.clone()).unwrap()
    };
    assert!(
        log.contains(&format!("request_id={resp_id}")),
        "expected request_id={resp_id} in log, got: {log}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn incoming_header_wins_over_generation() {
    let buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let _guard = init_capture(Arc::clone(&buf));

    async fn handler() -> &'static str {
        tracing::info!("test handler event");
        "ok"
    }

    let app = Router::new()
        .route("/test", get(handler))
        .layer(mediafusion_api::make_trace_layer!())
        .layer(axum::middleware::from_fn(
            mediafusion_api::request_id_middleware::request_id_middleware,
        ));

    let req = Request::builder()
        .uri("/test")
        .header("x-request-id", "external-42")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp_id = resp
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok());
    assert_eq!(resp_id, Some("external-42"));

    let log = {
        let b = buf.lock().unwrap();
        String::from_utf8(b.clone()).unwrap()
    };
    assert!(
        log.contains("request_id=external-42"),
        "expected request_id=external-42 in log, got: {log}"
    );
    // Ensure no freshly-generated UUID appears (32 hex chars)
    let uuid_pattern = log
        .lines()
        .any(|line| line.contains("request_id=") && !line.contains("external-42"));
    assert!(!uuid_pattern, "no freshly-generated UUID should appear");
}
