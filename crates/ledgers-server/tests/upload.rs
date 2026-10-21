//! Integration tests for document upload streaming.

use bytes::Bytes;
use cratefield_core::{
    UlidIdGen,
    axum::{
        self,
        body::Body,
        http::{HeaderValue, Method, Request, StatusCode, header},
        response::Response,
    },
};
use cratefield_testing::{MemoryBlob, TestHarness};
use futures_core::Stream;
use ledgers_server::documents::DocumentsModule;
use serde_json::Value;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tower::ServiceExt;

/// A chunked request body: a `Vec<Bytes>` drained one chunk per poll.
struct Chunks {
    items: std::vec::IntoIter<Bytes>,
}

impl Stream for Chunks {
    type Item = Result<Bytes, std::convert::Infallible>;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.get_mut().items.next().map(Ok))
    }
}

/// A simple test response wrapper
struct TestResponse {
    pub status: StatusCode,
    pub body: Bytes,
}

impl TestResponse {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).expect("valid JSON")
    }

    pub async fn from_response(response: Response) -> Self {
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        TestResponse { status, body }
    }
}

/// Helper to make a POST upload request with chunked body and custom content-type
async fn upload_with_content_type(
    harness: &TestHarness,
    chunks: Vec<Bytes>,
    content_type: &str,
) -> TestResponse {
    let body = Body::from_stream(Chunks {
        items: chunks.into_iter(),
    });
    let request = Request::builder()
        .method(Method::POST)
        .uri("/v1/documents/upload")
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_str(content_type).unwrap(),
        )
        .body(body)
        .expect("request builds");

    let response = harness
        .router
        .clone()
        .oneshot(request)
        .await
        .expect("router answers");
    TestResponse::from_response(response).await
}

/// Helper to make a POST upload request with chunked body (defaults to application/pdf)
async fn upload(harness: &TestHarness, chunks: Vec<Bytes>) -> TestResponse {
    upload_with_content_type(harness, chunks, "application/pdf").await
}

#[tokio::test]
async fn upload_1kb_pdf_succeeds() {
    let modules: Vec<Box<dyn cratefield_core::Module>> = vec![
        Box::new(ledgers_storage::StorageModule),
        Box::new(DocumentsModule::new()),
    ];
    let harness = TestHarness::with_ports(modules, |ports| {
        ports.blob = Some(Arc::new(MemoryBlob::new()));
        ports.id_gen = Some(Arc::new(UlidIdGen));
    });

    // Create a 1 KiB invented PDF
    let mut pdf_bytes = b"%PDF-1.7\n".to_vec();
    pdf_bytes.resize(1024, b'X');

    // Upload as single chunk
    let response = upload(&harness, vec![Bytes::from(pdf_bytes)]).await;

    assert_eq!(response.status, StatusCode::CREATED);
    let body = response.json();
    assert_eq!(body["bytes"], 1024);
    assert!(body["key"].as_str().unwrap().starts_with("documents/doc_"));
    assert_eq!(body["content_type"], "application/pdf");
}

#[tokio::test]
async fn upload_11mb_pdf_without_opt_in_returns_413() {
    let modules: Vec<Box<dyn cratefield_core::Module>> = vec![
        Box::new(ledgers_storage::StorageModule),
        Box::new(DocumentsModule::new()),
    ];
    let harness = TestHarness::with_ports(modules, |ports| {
        ports.blob = Some(Arc::new(MemoryBlob::new()));
        ports.id_gen = Some(Arc::new(UlidIdGen));
    });

    // Create an 11 MiB invented PDF (exceeds default 10 MiB cap)
    let mut pdf_bytes = b"%PDF-1.7\n".to_vec();
    pdf_bytes.resize(11 * 1024 * 1024, b'X');

    // Upload in multiple chunks to exercise streaming
    let chunks = pdf_bytes
        .chunks(1024 * 1024)
        .map(|c| Bytes::copy_from_slice(c))
        .collect();

    let response = upload(&harness, chunks).await;

    // Should get 413 Payload Too Large
    assert_eq!(response.status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn upload_html_returns_415() {
    let modules = vec![
        Box::new(ledgers_storage::StorageModule) as Box<dyn cratefield_core::Module>,
        Box::new(ledgers_server::documents::DocumentsModule::new()),
    ];
    let harness = TestHarness::with_ports(modules, |ports| {
        ports.blob = Some(Arc::new(MemoryBlob::new()));
        ports.id_gen = Some(Arc::new(UlidIdGen));
    });

    // Create some HTML content
    let html_bytes = b"<html><body>test</body></html>".to_vec();

    // Try to upload with unsupported content-type
    let response =
        upload_with_content_type(&harness, vec![Bytes::from(html_bytes)], "text/html").await;

    // Should get 415 Unsupported Media Type
    assert_eq!(response.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn upload_25mb_pdf_with_large_blob_opt_in_succeeds() {
    // Create a module with large blob enabled (25 MB is < 33 MB limit)
    struct TestConfig;
    impl cratefield_core::Config for TestConfig {
        fn get(&self, key: &str) -> Option<String> {
            if key == "DOCUMENTS_MAX_LARGE_BLOB_BYTES" {
                Some("33554432".to_string()) // 32 MiB
            } else {
                None
            }
        }
    }

    let modules = vec![
        Box::new(ledgers_storage::StorageModule) as Box<dyn cratefield_core::Module>,
        Box::new(ledgers_server::documents::DocumentsModule::from_config(&TestConfig).unwrap()),
    ];
    let harness = TestHarness::with_ports(modules, |ports| {
        ports.blob = Some(Arc::new(MemoryBlob::new()));
        ports.id_gen = Some(Arc::new(UlidIdGen));
    });

    // Create a 25 MiB invented PDF
    let mut pdf_bytes = b"%PDF-1.7\n".to_vec();
    pdf_bytes.resize(25_000_000, b'X');

    // Upload in multiple chunks
    let chunks = pdf_bytes
        .chunks(1024 * 1024)
        .map(|c| Bytes::copy_from_slice(c))
        .collect();

    let response = upload(&harness, chunks).await;

    assert_eq!(response.status, StatusCode::CREATED);
    let body = response.json();
    assert_eq!(body["bytes"], 25_000_000);
    assert!(body["key"].as_str().unwrap().starts_with("documents/doc_"));
    assert_eq!(body["content_type"], "application/pdf");
}

#[test]
fn module_config_validation() {
    // Test that invalid config is rejected
    struct InvalidConfig;
    impl cratefield_core::Config for InvalidConfig {
        fn get(&self, key: &str) -> Option<String> {
            if key == "DOCUMENTS_MAX_LARGE_BLOB_BYTES" {
                Some("not_a_number".to_string())
            } else {
                None
            }
        }
    }

    let result = ledgers_server::documents::DocumentsModule::from_config(&InvalidConfig);
    assert!(result.is_err());
}

#[test]
fn module_config_exceeds_ceiling() {
    // Test that values exceeding harness ceiling are rejected
    struct TooLargeConfig;
    impl cratefield_core::Config for TooLargeConfig {
        fn get(&self, key: &str) -> Option<String> {
            if key == "DOCUMENTS_MAX_LARGE_BLOB_BYTES" {
                // Much larger than harness MAX_LARGE_BLOB_BYTES (5 GiB)
                Some("99999999999999".to_string())
            } else {
                None
            }
        }
    }

    let result = ledgers_server::documents::DocumentsModule::from_config(&TooLargeConfig);
    assert!(result.is_err());
}
