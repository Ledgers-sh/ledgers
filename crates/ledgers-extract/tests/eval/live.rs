#![allow(clippy::too_many_lines)]

// Live extraction evaluation against a real model.
// Ignored by default; run with `--ignored` to execute.
//
// Requires configuration via environment variables:
// - `LEDGERS_EXTRACT_PROVIDER`: "anthropic" or "minimax" (required)
// - `LEDGERS_EXTRACT_API_KEY`: Anthropic API key (if provider is anthropic)
// - `LEDGERS_MINIMAX_API_KEY`: MiniMax API key (if provider is minimax)
// - `LEDGERS_MINIMAX_MODEL`: MiniMax model id (optional)
// - `LEDGERS_EVAL_OUT`: output file path for report (default: stdout)
// - `LEDGERS_EVAL_PRICE_IN_UUSD_PER_MTOK`: input token price (in microUSD per million tokens)
// - `LEDGERS_EVAL_PRICE_OUT_UUSD_PER_MTOK`: output token price (in microUSD per million tokens)
// - `LEDGERS_EVAL_RECORD`: if "1", also write recordings to LEDGERS_EVAL_OUT.recorded.json

use ledgers_extract::DocumentInput;
use ledgers_extract::llm::Transport;
use ledgers_extract::minimax::Pause;
use ledgers_extract::provider;
use ledgers_extract::schema::ExtractedInvoice;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Extracts usage (input, output) tokens from an API response.
/// Handles both Anthropic format (`usage.input_tokens`, `usage.output_tokens`)
/// and OpenAI-style format (`usage.prompt_tokens`, `usage.completion_tokens`).
/// Returns (0, 0) if usage is missing or malformed.
fn usage(response: &Value) -> (u64, u64) {
    if let Some(usage) = response.get("usage") {
        // Try Anthropic format first
        if let (Some(input), Some(output)) = (
            usage.get("input_tokens").and_then(Value::as_u64),
            usage.get("output_tokens").and_then(Value::as_u64),
        ) {
            return (input, output);
        }
        // Try OpenAI-style format
        if let (Some(prompt), Some(completion)) = (
            usage.get("prompt_tokens").and_then(Value::as_u64),
            usage.get("completion_tokens").and_then(Value::as_u64),
        ) {
            return (prompt, completion);
        }
    }
    (0, 0)
}

/// Computes cost in microUSD: (`input_tokens` × `price_in_uusd_per_mtok` + `output_tokens` × `price_out_uusd_per_mtok`) / `1_000_000`
fn cost_uusd(
    input_tokens: u64,
    output_tokens: u64,
    price_in_uusd_per_mtok: u128,
    price_out_uusd_per_mtok: u128,
) -> u128 {
    ((u128::from(input_tokens) * price_in_uusd_per_mtok)
        + (u128::from(output_tokens) * price_out_uusd_per_mtok))
        / 1_000_000
}

/// Formats microUSD as a decimal USD string with 6 decimal places.
fn usd(uusd: u128) -> String {
    let whole = uusd / 1_000_000;
    let frac = uusd % 1_000_000;
    format!("{whole}.{frac:0>6} USD")
}

/// A transport wrapper that meters all requests by counting calls and capturing usage tokens.
struct Metered {
    inner: Arc<dyn Transport>,
    call_count: AtomicU64,
    input_tokens: AtomicU64,
    output_tokens: AtomicU64,
}

#[async_trait::async_trait]
impl Transport for Metered {
    async fn post_json(
        &self,
        url: &str,
        headers: &[(&str, String)],
        body: &Value,
    ) -> Result<(u16, Value), String> {
        let (status, response) = self.inner.post_json(url, headers, body).await?;
        self.call_count.fetch_add(1, Ordering::SeqCst);
        let (in_tok, out_tok) = usage(&response);
        self.input_tokens.fetch_add(in_tok, Ordering::SeqCst);
        self.output_tokens.fetch_add(out_tok, Ordering::SeqCst);
        Ok((status, response))
    }
}

impl Metered {
    fn new(inner: Arc<dyn Transport>) -> Self {
        Self {
            inner,
            call_count: AtomicU64::new(0),
            input_tokens: AtomicU64::new(0),
            output_tokens: AtomicU64::new(0),
        }
    }

    fn calls(&self) -> u64 {
        self.call_count.load(Ordering::SeqCst)
    }

    fn input_tokens(&self) -> u64 {
        self.input_tokens.load(Ordering::SeqCst)
    }

    fn output_tokens(&self) -> u64 {
        self.output_tokens.load(Ordering::SeqCst)
    }
}

/// The real blocking HTTP client for live calls.
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

/// A simple pause implementation using `thread::sleep`.
struct Sleep;

#[async_trait::async_trait]
impl Pause for Sleep {
    async fn pause(&self, delay: Duration) {
        std::thread::sleep(delay);
    }
}

/// Corpus case definition (reuse from parent module).
#[derive(serde::Deserialize, Clone)]
struct CorpusCase {
    id: String,
    #[serde(default)]
    medium: String,
    text: String,
    #[serde(default)]
    adversarial: Option<serde_json::Value>,
}

/// Loads the test corpus from the embedded corpus.json.
fn load_corpus() -> Vec<CorpusCase> {
    #[derive(serde::Deserialize)]
    struct Corpus {
        cases: Vec<CorpusCase>,
    }
    let corpus: Corpus =
        serde_json::from_str(include_str!("corpus.json")).expect("valid corpus JSON");
    corpus.cases
}

/// Live extraction evaluation against a real model.
/// Ignored by default; run with `--ignored` to execute.
#[tokio::test]
#[ignore = "live: needs LEDGERS_EXTRACT_PROVIDER and API key; costs real quota"]
async fn live_eval() {
    // Load environment configuration
    let provider_name = std::env::var("LEDGERS_EXTRACT_PROVIDER").unwrap_or_else(|_| {
        panic!(
            "live_eval requires:\n  \
                 LEDGERS_EXTRACT_PROVIDER=anthropic|minimax\n  \
                 LEDGERS_EXTRACT_API_KEY=<anthropic-key> (for anthropic)\n  \
                 LEDGERS_MINIMAX_API_KEY=<minimax-key> (for minimax)"
        )
    });

    let anthropic_key = std::env::var("LEDGERS_EXTRACT_API_KEY").ok();
    let minimax_key = std::env::var("LEDGERS_MINIMAX_API_KEY").ok();
    let minimax_model = std::env::var("LEDGERS_MINIMAX_MODEL").ok();

    // Call provider::select to get the model settings
    let selection = provider::select(
        Some(&provider_name),
        anthropic_key.as_deref(),
        minimax_key.as_deref(),
        minimax_model.as_deref(),
    );

    let settings = selection.settings.as_ref().unwrap_or_else(|| {
        panic!(
            "live_eval: provider selection failed. Provider: {}, Status: {:?}",
            selection.status.provider, selection.status.reason
        )
    });

    // Build the HTTP client and metered transport
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .expect("client");
    let live = Arc::new(Live { client });
    let metered = Arc::new(Metered::new(live));

    // Build the extractor
    let extractor = settings.extractor(metered.clone(), Arc::new(Sleep));

    // Load corpus
    let corpus = load_corpus();
    let non_adv: Vec<_> = corpus.iter().filter(|c| c.adversarial.is_none()).collect();

    // Track results
    let mut run_cases = Vec::new();
    let mut skipped_cases = Vec::new();
    let mut errored_cases: Vec<(String, String)> = Vec::new();
    let mut recordings: BTreeMap<String, ExtractedInvoice> = BTreeMap::new();

    // Process each case
    for case in &non_adv {
        // For PDFs with text, use the text. For scans/images, look for a local image file.
        let doc_result = if case.medium == "scan" || case.medium == "photo" {
            // Look for an image file
            let png_path = format!(
                "{}/tests/eval/images/{}.png",
                env!("CARGO_MANIFEST_DIR"),
                case.id
            );
            let jpg_path = format!(
                "{}/tests/eval/images/{}.jpg",
                env!("CARGO_MANIFEST_DIR"),
                case.id
            );

            if std::path::Path::new(&png_path).exists() {
                let bytes = std::fs::read(&png_path).expect("read PNG");
                Some((bytes, "image/png"))
            } else if std::path::Path::new(&jpg_path).exists() {
                let bytes = std::fs::read(&jpg_path).expect("read JPEG");
                Some((bytes, "image/jpeg"))
            } else {
                skipped_cases.push(case.id.clone());
                None
            }
        } else {
            // Clean PDF from text
            let pdf = ledgers_extract::fixture::pdf_with_text(&[&case.text]);
            Some((pdf, "application/pdf"))
        };

        if let Some((bytes, mime)) = doc_result {
            let doc = DocumentInput {
                bytes: &bytes,
                mime,
                text_layer: if mime == "application/pdf" {
                    Some(&case.text)
                } else {
                    None
                },
            };

            // Extract
            match pollster::block_on(extractor.extract(&doc)) {
                Ok(extraction) => {
                    run_cases.push(case.id.clone());

                    // Record if requested
                    if std::env::var("LEDGERS_EVAL_RECORD").is_ok_and(|v| v == "1") {
                        recordings.insert(case.id.clone(), extraction.invoice.clone());
                    }

                    // For live runs, we don't have expected values, so we don't score against a
                    // baseline. The report is informational only. We could score here for
                    // reference, but it's not needed for the live evaluation report.
                }
                Err(e) => {
                    errored_cases.push((case.id.clone(), e.to_string()));
                }
            }
        }
    }

    // Check that at least some cases ran
    assert!(
        !run_cases.is_empty() || !errored_cases.is_empty(),
        "live_eval: no cases were attempted. Check that corpus.json has non-adversarial cases and images are present."
    );

    // Get metrics
    let calls = metered.calls();
    let input_tokens = metered.input_tokens();
    let output_tokens = metered.output_tokens();

    // Parse pricing
    let price_in = std::env::var("LEDGERS_EVAL_PRICE_IN_UUSD_PER_MTOK")
        .ok()
        .and_then(|s| s.parse::<u128>().ok());
    let price_out = std::env::var("LEDGERS_EVAL_PRICE_OUT_UUSD_PER_MTOK")
        .ok()
        .and_then(|s| s.parse::<u128>().ok());

    // Generate report
    let mut report = String::new();
    writeln!(&mut report, "# Extraction Evaluation Report\n").ok();
    writeln!(&mut report, "## Live run\n").ok();
    writeln!(&mut report, "**Provider**: {}\n", selection.status.provider).ok();
    writeln!(&mut report, "**Model**: {}\n", settings.model).ok();
    writeln!(
        &mut report,
        "**Cases run**: {} ({:?})\n",
        run_cases.len(),
        run_cases
    )
    .ok();

    if !skipped_cases.is_empty() {
        writeln!(
            &mut report,
            "**Cases skipped** (missing image files): {} ({:?})\n",
            skipped_cases.len(),
            skipped_cases
        )
        .ok();
    }

    if !errored_cases.is_empty() {
        writeln!(&mut report, "**Cases errored**:\n").ok();
        for (id, error) in &errored_cases {
            writeln!(&mut report, "- {id}: {error}").ok();
        }
        writeln!(&mut report).ok();
    }

    writeln!(&mut report, "**API calls**: {calls}\n").ok();
    writeln!(&mut report, "**Input tokens**: {input_tokens}\n").ok();
    writeln!(&mut report, "**Output tokens**: {output_tokens}\n").ok();

    // Cost calculation
    if let (Some(pin), Some(pout)) = (price_in, price_out) {
        let cost = cost_uusd(input_tokens, output_tokens, pin, pout);
        writeln!(&mut report, "**Cost**: {}\n", usd(cost)).ok();
    } else {
        writeln!(&mut report, "**Cost**: prices not configured\n").ok();
    }

    writeln!(
        &mut report,
        "**Note**: Live run is informational only and does not update the baseline.\n"
    )
    .ok();

    // Write report
    if let Ok(out_path) = std::env::var("LEDGERS_EVAL_OUT") {
        std::fs::write(&out_path, &report).expect("write report");
        eprintln!("Report written to: {out_path}");

        // Write recordings if requested
        if std::env::var("LEDGERS_EVAL_RECORD").is_ok_and(|v| v == "1") && !recordings.is_empty() {
            let recordings_json = serde_json::to_string_pretty(&recordings).expect("serialize");
            let recordings_path = format!("{out_path}.recorded.json");
            std::fs::write(&recordings_path, &recordings_json).expect("write recordings");
            eprintln!("Recordings written to: {recordings_path}");
        }
    } else {
        println!("{report}");
        if std::env::var("LEDGERS_EVAL_RECORD").is_ok_and(|v| v == "1") && !recordings.is_empty() {
            println!("\n## Recordings (for corpus.json):\n");
            println!(
                "{}",
                serde_json::to_string_pretty(&recordings).expect("serialize")
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_parses_anthropic_format() {
        let response = serde_json::json!({
            "usage": {
                "input_tokens": 100,
                "output_tokens": 50
            }
        });
        assert_eq!(usage(&response), (100, 50));
    }

    #[test]
    fn usage_parses_openai_format() {
        let response = serde_json::json!({
            "usage": {
                "prompt_tokens": 200,
                "completion_tokens": 75
            }
        });
        assert_eq!(usage(&response), (200, 75));
    }

    #[test]
    fn usage_returns_zero_on_missing() {
        let response = serde_json::json!({});
        assert_eq!(usage(&response), (0, 0));
    }

    #[test]
    fn usage_returns_zero_on_malformed() {
        let response = serde_json::json!({
            "usage": {
                "input_tokens": "not_a_number"
            }
        });
        assert_eq!(usage(&response), (0, 0));
    }

    #[test]
    fn cost_calculation_matches_spec() {
        // cost_uusd(1_234_567, 89_000, 1_000_000, 5_000_000) = 1_679_567
        let cost = cost_uusd(1_234_567, 89_000, 1_000_000, 5_000_000);
        assert_eq!(cost, 1_679_567);
    }

    #[test]
    fn usd_formatting() {
        assert_eq!(usd(1_679_567), "1.679567 USD");
        assert_eq!(usd(5), "0.000005 USD");
        assert_eq!(usd(1_000_000), "1.000000 USD");
    }
}
