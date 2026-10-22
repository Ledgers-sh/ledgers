//! A live call to `MiniMax`'s chat API. Ignored in CI: it needs a real key and
//! network access, and it spends quota.
//!
//! Run it with the key passed through the environment only:
//!
//! ```text
//! LEDGERS_MINIMAX_API_KEY="$(cat <keyfile>)" \
//!   cargo test -p ledgers-extract --test minimax_live -- --ignored --nocapture
//! ```
//!
//! Optional: `LEDGERS_MINIMAX_MODEL` overrides the model id, and
//! `LEDGERS_MINIMAX_TEST_IMAGE` names a local JPEG or PNG invoice to read as
//! the image case (the repository has no invoice image, only a blank scan).
//!
//! Nothing here prints the key. Failures show the HTTP status and the error
//! kind only.

use ledgers_extract::llm::Transport;
use ledgers_extract::minimax::{self, MiniMaxExtractor, Pause};
use ledgers_extract::pdf;
use ledgers_extract::{DocumentInput, Extractor, Reader};
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The blocking HTTP call the live check makes.
struct Live {
    client: reqwest::blocking::Client,
}

#[async_trait::async_trait]
impl Transport for Live {
    async fn post_json(
        &self,
        url: &str,
        headers: &[(&str, String)],
        body: &Value,
    ) -> Result<(u16, Value), String> {
        let mut request = self.client.post(url).json(body);
        for (name, value) in headers {
            request = request.header(*name, value.as_str());
        }
        let response = request
            .send()
            .map_err(|err| format!("transport error: {}", err.without_url()))?;
        let status = response.status().as_u16();
        let text = response
            .text()
            .map_err(|err| format!("transport error: {}", err.without_url()))?;
        Ok((status, serde_json::from_str(&text).unwrap_or(Value::Null)))
    }
}

/// The real pause: a thread sleep, acceptable in a test.
struct Sleep;

#[async_trait::async_trait]
impl Pause for Sleep {
    async fn pause(&self, delay: Duration) {
        std::thread::sleep(delay);
    }
}

fn key() -> String {
    std::env::var("LEDGERS_MINIMAX_API_KEY")
        .expect("set LEDGERS_MINIMAX_API_KEY in the environment (never in a file in the repo)")
}

fn model() -> String {
    std::env::var("LEDGERS_MINIMAX_MODEL").unwrap_or_else(|_| minimax::MODEL.to_owned())
}

fn extractor() -> MiniMaxExtractor {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .expect("client");
    MiniMaxExtractor::new(key(), model(), Arc::new(Live { client }), Arc::new(Sleep))
}

/// Runs one extraction and reports the outcome and latency without the key.
fn run(doc: &DocumentInput<'_>) -> Result<ledgers_extract::Extraction, String> {
    let started = Instant::now();
    let result = pollster::block_on(extractor().extract(doc));
    eprintln!(
        "minimax model={} mime={} latency_ms={}",
        model(),
        doc.mime,
        started.elapsed().as_millis()
    );
    result.map_err(|err| err.to_string())
}

#[test]
#[ignore = "live: needs LEDGERS_MINIMAX_API_KEY and network"]
fn the_text_layer_pdf_reads_its_totals() {
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../app/tests/fixtures/bills/invoice.pdf"
    ))
    .expect("fixture PDF");
    let text = pdf::text_layer(&bytes)
        .expect("parses")
        .expect("text layer");
    let doc = DocumentInput {
        bytes: &bytes,
        mime: "application/pdf",
        text_layer: Some(&text),
    };
    let extraction = run(&doc).expect("MiniMax read the text layer");
    assert_eq!(extraction.reader, Reader::MiniMax);
    assert_eq!(extraction.invoice.net_minor, Some(100_000));
    assert_eq!(extraction.invoice.vat_minor, Some(19_000));
    assert_eq!(extraction.invoice.gross_minor, Some(119_000));
    assert_eq!(extraction.invoice.currency.as_deref(), Some("EUR"));
}

#[test]
#[ignore = "live: needs LEDGERS_MINIMAX_API_KEY, network and LEDGERS_MINIMAX_TEST_IMAGE"]
fn an_invoice_image_reads_its_totals() {
    let path = std::env::var("LEDGERS_MINIMAX_TEST_IMAGE")
        .expect("set LEDGERS_MINIMAX_TEST_IMAGE to a local invoice JPEG or PNG");
    let bytes = std::fs::read(&path).expect("readable image");
    let mime = if std::path::Path::new(&path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
    {
        "image/png"
    } else {
        "image/jpeg"
    };
    let doc = DocumentInput {
        bytes: &bytes,
        mime,
        text_layer: None,
    };
    let extraction = run(&doc).expect("MiniMax read the image");
    assert_eq!(extraction.reader, Reader::MiniMax);
    assert_eq!(extraction.invoice.gross_minor, Some(119_000));
    assert_eq!(extraction.invoice.net_minor, Some(100_000));
    assert_eq!(extraction.invoice.vat_minor, Some(19_000));
}

#[test]
#[ignore = "live: needs LEDGERS_MINIMAX_API_KEY and network"]
fn a_blank_scan_is_not_given_invented_totals() {
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../app/tests/fixtures/bills/scan.png"
    ))
    .expect("fixture scan");
    let doc = DocumentInput {
        bytes: &bytes,
        mime: "image/png",
        text_layer: None,
    };
    let extraction = run(&doc).expect("the reply is schema-valid");
    assert_eq!(
        extraction.invoice.gross_minor, None,
        "a blank scan has no total"
    );
}
