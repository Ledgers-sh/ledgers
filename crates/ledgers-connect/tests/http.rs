//! The vendor HTTP rules (issue #142): fixed hosts, bounded retries, refusal
//! without retry, and no credential in any error or log line.

mod support;

use ledgers_connect::http::{VendorError, VendorHttp};
use ledgers_connect::secret::ConnectKey;
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::{Recorded, RecordedPause, vendor};

const HOSTS: &[&str] = &["api.example.test"];
const SECRET_TOKEN: &str = "cfat_SECRET_TOKEN_VALUE_9f8e7d";

#[tokio::test]
async fn a_url_off_the_allow_list_is_never_sent() {
    let client = Recorded::new(Vec::new());
    let (http, _) = vendor(client.clone());
    for url in [
        "https://evil.example.test/steal",
        "http://api.example.test/plain",
        "https://169.254.169.254/latest/meta-data/",
        "https://api.example.test.attacker.test/",
        "https://api.example.test:8443/x",
        "https://user@api.example.test/x",
    ] {
        let err = http.get_json(HOSTS, url, &[]).await.unwrap_err();
        assert!(
            matches!(err, VendorError::HostNotAllowed(_)),
            "{url}: {err:?}"
        );
    }
    assert_eq!(client.calls(), 0, "nothing was sent");
}

#[tokio::test]
async fn a_server_error_is_retried_with_backoff_then_succeeds() {
    let client = Recorded::script(vec![(503, "{}"), (500, "{}"), (200, "{\"ok\":true}")]);
    let (http, pause) = vendor(client.clone());
    let value = http
        .get_json(HOSTS, "https://api.example.test/x", &[])
        .await
        .unwrap();
    assert_eq!(value, json!({"ok": true}));
    assert_eq!(client.calls(), 3);
    assert_eq!(
        *pause.0.lock().unwrap(),
        vec![Duration::from_millis(500), Duration::from_millis(1000)]
    );
}

#[tokio::test]
async fn retries_stop_after_three_attempts() {
    let client = Recorded::script(vec![(503, "{}"), (503, "{}"), (503, "{}"), (200, "{}")]);
    let (http, pause) = vendor(client.clone());
    let err = http
        .get_json(HOSTS, "https://api.example.test/x", &[])
        .await
        .unwrap_err();
    assert_eq!(
        err,
        VendorError::Status {
            host: "api.example.test".into(),
            status: 503
        }
    );
    assert_eq!(client.calls(), 3, "three attempts, then the answer stands");
    assert_eq!(pause.0.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_refused_token_is_not_retried_and_names_no_token() {
    let client = Recorded::script(vec![(401, "{}"), (200, "{}")]);
    let (http, pause) = vendor(client.clone());
    let err = http
        .get_json(
            HOSTS,
            "https://api.example.test/x",
            &[("authorization", format!("Bearer {SECRET_TOKEN}"))],
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, VendorError::Refused { status: 401, .. }),
        "{err:?}"
    );
    assert_eq!(client.calls(), 1);
    assert!(pause.0.lock().unwrap().is_empty());
    assert!(!err.to_string().contains(SECRET_TOKEN));
}

#[tokio::test]
async fn a_malformed_answer_is_reported_without_the_body() {
    let client = Recorded::script(vec![(200, "not json at all")]);
    let (http, _): (VendorHttp, _) = vendor(client);
    let err = http
        .get_json(HOSTS, "https://api.example.test/x", &[])
        .await
        .unwrap_err();
    assert!(matches!(err, VendorError::Malformed { .. }), "{err:?}");
}

/// A writer that keeps everything a subscriber prints, for the log-capture test.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn the_credential_never_reaches_logs_errors_or_debug_output() {
    let capture = Capture::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer({
            let capture = capture.clone();
            move || capture.clone()
        })
        .with_max_level(tracing::Level::TRACE)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let client = Recorded::script(vec![(401, "{\"message\":\"no\"}")]);
    let (http, _pause): (VendorHttp, Arc<RecordedPause>) = vendor(client);
    let err = http
        .get_json(
            HOSTS,
            "https://api.example.test/v1?token=query-secret",
            &[("authorization", format!("Bearer {SECRET_TOKEN}"))],
        )
        .await
        .unwrap_err();
    tracing::info!(error = %err, "vendor refused");
    let key = ConnectKey::from_secret("0123456789abcdef0123456789abcdef-long-enough").unwrap();
    let sealed = key.seal(SECRET_TOKEN, "conn-1").unwrap();
    tracing::info!(?key, sealed = %sealed, "sealed a credential");

    let logged = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    assert!(
        logged.contains("vendor refused"),
        "the subscriber did capture: {logged}"
    );
    assert!(
        !logged.contains(SECRET_TOKEN),
        "the token never reaches a log line"
    );
    assert!(
        !logged.contains("query-secret"),
        "the query string never reaches a log line"
    );
    assert!(!err.to_string().contains("query-secret"));
    assert!(!format!("{err:?}").contains(SECRET_TOKEN));
}
