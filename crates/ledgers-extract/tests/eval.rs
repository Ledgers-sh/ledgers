//! Extraction evaluation: accuracy report against a corpus of recorded model replies.
//! Run with `LEDGERS_EVAL_BLESS=1 cargo test` to regenerate the report.
//!
//! Live testing (real model, real costs) is the `live_eval` test below.
#![allow(
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    clippy::manual_checked_ops,
    clippy::collapsible_if
)]

// Live evaluation module
#[path = "eval/live.rs"]
mod live;

use ledgers_extract::harness::HarnessExtractor;
use ledgers_extract::schema::ExtractedInvoice;
use ledgers_extract::{DocumentInput, Extractor, Reader};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Debug, Serialize, Deserialize, Clone)]
struct CorpusCase {
    id: String,
    lang: String,
    kind: String,
    medium: String,
    text: String,
    expected: BTreeMap<String, serde_json::Value>,
    recorded: RecordedReplies,
    #[serde(default)]
    adversarial: Option<AdversarialExpectation>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct RecordedReplies {
    fast: ExtractedInvoice,
    strong: Option<ExtractedInvoice>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct AdversarialExpectation {
    #[serde(default)]
    guard_flags_expected: Option<Vec<String>>,
    #[serde(default)]
    check_flags_expected: Option<Vec<String>>,
    #[serde(default)]
    fields_expected: Option<BTreeMap<String, serde_json::Value>>,
    #[serde(default)]
    expect_error: Option<String>,
    #[serde(default)]
    model_calls_expected: Option<u32>,
    #[serde(default)]
    note: Option<String>,
}

// Confidence buckets: (lower inclusive, upper inclusive)
const BUCKETS: [(u8, u8); 5] = [(0, 59), (60, 69), (70, 79), (80, 89), (90, 100)];

#[derive(Clone, Debug)]
struct ReplayModel {
    case_id: String,
    fast_reply: ExtractedInvoice,
    strong_reply: Option<ExtractedInvoice>,
    recorded_prompts: Arc<std::sync::Mutex<Vec<cratefield_core::Prompt>>>,
}

#[async_trait::async_trait]
impl cratefield_core::TextModel for ReplayModel {
    async fn complete(
        &self,
        p: &cratefield_core::Prompt,
    ) -> Result<cratefield_core::Completion, cratefield_core::TextModelError> {
        self.recorded_prompts.lock().unwrap().push(p.clone());

        // Decide tier based on confidence threshold, matching the real behavior in harness.rs
        let reply = if p.tier == cratefield_core::ModelTier::Strong {
            self.strong_reply
                .clone()
                .unwrap_or_else(|| self.fast_reply.clone())
        } else {
            self.fast_reply.clone()
        };
        let j = serde_json::json!(reply);
        Ok(cratefield_core::Completion::new(j.to_string(), &self.case_id).json(j))
    }

    fn stream<'a>(
        &'a self,
        _: &'a cratefield_core::Prompt,
    ) -> cratefield_core::BoxStream<
        'a,
        Result<cratefield_core::TextDelta, cratefield_core::TextModelError>,
    > {
        Box::pin(futures_util::stream::empty())
    }

    fn supports(&self, _: cratefield_core::ModelTier, _: cratefield_core::Capability) -> bool {
        true
    }
}

#[derive(Debug, Clone)]
struct Tally {
    total: u32,
    hits: u32,
}

#[derive(Debug)]
struct Metrics {
    /// (field/lang/kind/medium) -> (total, hits)
    by_field: BTreeMap<String, Tally>,
    /// bucket index -> (count, matched, `confidence_sum`); sum is in percentage (0-100)
    calibration: BTreeMap<u32, (u32, u32, u32)>,
}

fn pct(bp: u32) -> String {
    let whole = bp / 100;
    let frac = bp % 100;
    format!("{whole}.{frac:0>2} %")
}

fn load_corpus() -> Vec<CorpusCase> {
    #[derive(Deserialize)]
    struct Corpus {
        cases: Vec<CorpusCase>,
    }
    let corpus: Corpus =
        serde_json::from_str(include_str!("eval/corpus.json")).expect("valid corpus JSON");
    corpus.cases
}

fn normalize_string(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn render_report(
    metrics: &Metrics,
    corpus: &[CorpusCase],
    overall_accuracy_bp: u32,
    ece_bp: u32,
    case_count: u32,
) -> String {
    use std::fmt::Write;
    let mut report = String::new();

    writeln!(
        &mut report,
        "# Extraction Evaluation Report\n\n## Overview\n\n\
         Accuracy evaluation against {} recorded cases.\n\n\
         Regenerate with: `LEDGERS_EVAL_BLESS=1 cargo test -p ledgers-extract --test eval recorded_set`\n\n\
         Gate: Overall accuracy must be ≥baseline and per-field minimums must be met.\n\
         Baseline is only raised by hand in baseline.json.\n\n\
         Live testing (real model, real costs): trigger the \"Extraction eval\" workflow (`.github/workflows/extraction-eval.yml`) or run:\n\
         `cargo test -p ledgers-extract --test eval -- --ignored live_eval --nocapture`\n\
         with env vars: `LEDGERS_EXTRACT_PROVIDER`, `LEDGERS_EXTRACT_API_KEY`, `LEDGERS_EVAL_PRICE_IN_UUSD_PER_MTOK`, `LEDGERS_EVAL_PRICE_OUT_UUSD_PER_MTOK`\n\n\
         ## Summary\n\n\
         - **Cases**: {} (non-adversarial)\n\
         - **Overall Accuracy**: {} ({}bp)\n\
         - **ECE (Expected Calibration Error)**: {}bp\n",
        case_count,
        case_count,
        pct(overall_accuracy_bp),
        overall_accuracy_bp,
        ece_bp
    )
    .ok();

    writeln!(
        &mut report,
        "\n## Per-Field Accuracy\n\n| Field | Accuracy | Cases | Matched |\n|---|---|---|---|"
    )
    .ok();
    for field in [
        "vendor_name",
        "vendor_vat_id",
        "vendor_iban",
        "invoice_number",
        "invoice_date",
        "due_date",
        "currency",
        "net_minor",
        "vat_minor",
        "gross_minor",
    ] {
        if let Some(tally) = metrics.by_field.get(field) {
            let acc_bp = (tally.hits * 10000) / tally.total;
            writeln!(
                &mut report,
                "| {field} | {} | {} | {} |",
                pct(acc_bp),
                tally.total,
                tally.hits
            )
            .ok();
        }
    }

    writeln!(
        &mut report,
        "\n## Calibration Table\n\n| Bucket | Cases | Mean Conf | Observed Acc | Gap |"
    )
    .ok();
    writeln!(&mut report, "|---|---|---|---|---|").ok();
    for (bucket_idx, &(lower, upper)) in BUCKETS.iter().enumerate() {
        if let Some((count, matched, conf_sum)) = metrics.calibration.get(&(bucket_idx as u32)) {
            if *count > 0 {
                let mean_conf = *conf_sum / *count;
                let obs_acc = (*matched * 100) / *count;
                let gap = mean_conf.abs_diff(obs_acc);
                writeln!(
                    &mut report,
                    "| {lower}-{upper} | {count} | {mean_conf}% | {obs_acc}% | {gap}% |"
                )
                .ok();
            }
        }
    }

    writeln!(&mut report, "\n```mermaid").ok();
    writeln!(&mut report, "xychart-beta").ok();
    writeln!(&mut report, "    title \"Confidence calibration\"").ok();
    writeln!(
        &mut report,
        "    x-axis [\"0-59\", \"60-69\", \"70-79\", \"80-89\", \"90-100\"]"
    )
    .ok();
    writeln!(&mut report, "    y-axis \"Percent\" 0 --> 100").ok();

    write!(&mut report, "    bar [").ok();
    for bucket_idx in 0..5 {
        if bucket_idx > 0 {
            write!(&mut report, ", ").ok();
        }
        if let Some((count, matched, _)) = metrics.calibration.get(&bucket_idx) {
            if *count > 0 {
                let obs_pct = (*matched * 100) / *count;
                write!(&mut report, "{obs_pct}").ok();
            }
        }
    }
    writeln!(&mut report, "]").ok();

    write!(&mut report, "    line [").ok();
    for bucket_idx in 0..5 {
        if bucket_idx > 0 {
            write!(&mut report, ", ").ok();
        }
        if let Some((count, _, conf_sum)) = metrics.calibration.get(&bucket_idx) {
            if *count > 0 {
                let mean_conf = *conf_sum / *count;
                write!(&mut report, "{mean_conf}").ok();
            }
        }
    }
    writeln!(&mut report, "]").ok();
    writeln!(&mut report, "```").ok();

    writeln!(
        &mut report,
        "\n## Adversarial Cases\n\n| Case | Threat | Expectation | Note |"
    )
    .ok();
    writeln!(&mut report, "|---|---|---|---|").ok();
    for case in corpus.iter().filter(|c| c.adversarial.is_some()) {
        let adv = case.adversarial.as_ref().unwrap();
        let (threat, expectation) = match case.id.as_str() {
            "adv_injection_attempt" => (
                "Instruction injection".to_string(),
                "Guard flags fire ('ignore the above')".to_string(),
            ),
            "adv_markup_escape" => (
                "Prompt markup escape".to_string(),
                "Guard flags fire ('ignore previous' and '</document')".to_string(),
            ),
            "adv_white_text_iban" => (
                "Hidden text IBAN swap".to_string(),
                format!(
                    "vendor_iban = '{}' (visible text, not hidden)",
                    case.recorded.fast.vendor_iban.as_deref().unwrap_or("null")
                ),
            ),
            "adv_confidence_ask" => (
                "Confidence inflation request".to_string(),
                "Guard does not flag (empty guard_flags_expected)".to_string(),
            ),
            "adv_pdf_no_text" => (
                "PDF without text layer".to_string(),
                "error = NeedsPhoto".to_string(),
            ),
            _ => ("Unknown".to_string(), "Unknown expectation".to_string()),
        };
        let note = adv.note.as_deref().unwrap_or("No note");
        writeln!(
            &mut report,
            "| {} | {} | {} | {} |",
            case.id, threat, expectation, note
        )
        .ok();
    }

    report
}

#[tokio::test]
async fn recorded_set() {
    let corpus = load_corpus();
    let non_adv: Vec<_> = corpus.iter().filter(|c| c.adversarial.is_none()).collect();

    let mut metrics = Metrics {
        by_field: BTreeMap::new(),
        calibration: BTreeMap::new(),
    };

    let mut total_hits = 0u32;
    let mut total_count = 0u32;

    for case in &non_adv {
        let model = ReplayModel {
            case_id: case.id.clone(),
            fast_reply: case.recorded.fast.clone(),
            strong_reply: case.recorded.strong.clone(),
            recorded_prompts: Arc::new(std::sync::Mutex::new(Vec::new())),
        };

        // Build the document input
        let pdf = ledgers_extract::fixture::pdf_with_text(&[&case.text]);
        let doc = DocumentInput {
            bytes: &pdf,
            mime: "application/pdf",
            text_layer: Some(&case.text),
        };

        // Universal invariant will be checked in prompts: exactly one [document_text] wrapper
        // (angle brackets neutralized to square brackets) in user message, never in system.
        // This is verified below when we check prompts.

        let extractor = HarnessExtractor::new(Arc::new(model.clone()), Reader::Anthropic);
        let result = extractor.extract(&doc).await;

        match result {
            Ok(extraction) => {
                let prompts = model.recorded_prompts.lock().unwrap();

                // Check: system prompt never contains document text
                for prompt in prompts.iter() {
                    if let Some(system) = &prompt.system {
                        assert!(
                            !system.contains(&case.text),
                            "Case {}: document text in system prompt",
                            case.id
                        );
                    }
                }

                // Score fields
                for field in case.expected.keys() {
                    let expected = &case.expected[field];
                    let got = get_extracted_value(&extraction.invoice, field);

                    let matched = match (expected, got) {
                        (serde_json::Value::Null, None) => true,
                        (serde_json::Value::String(e), Some(v)) => normalize_string(e) == v,
                        (serde_json::Value::Number(e), Some(v)) => {
                            e.as_i64() == v.trim().parse::<i64>().ok()
                        }
                        _ => false,
                    };

                    let conf = extraction
                        .invoice
                        .confidence
                        .get(field)
                        .copied()
                        .unwrap_or(0);

                    let tally = metrics
                        .by_field
                        .entry(field.clone())
                        .or_insert(Tally { total: 0, hits: 0 });
                    tally.total += 1;
                    if matched {
                        tally.hits += 1;
                    }
                    total_count += 1;
                    if matched {
                        total_hits += 1;
                    }

                    // Bucket for calibration
                    let bucket = BUCKETS
                        .iter()
                        .position(|(l, u)| conf >= *l && conf <= *u)
                        .unwrap_or(0) as u32;
                    let (count, matched_count, conf_sum) =
                        metrics.calibration.entry(bucket).or_insert((0, 0, 0));
                    *count += 1;
                    if matched {
                        *matched_count += 1;
                    }
                    *conf_sum += u32::from(conf);
                }
            }
            Err(e) => {
                panic!("Case {}: unexpected extraction error: {:?}", case.id, e);
            }
        }
    }

    // Calculate overall accuracy in basis points
    let overall_accuracy_bp = if total_count > 0 {
        (total_hits * 10000) / total_count
    } else {
        0
    };

    // Calculate ECE: Σ_b |hits_b·10000 − sum_conf_b·100| / N
    let mut ece_contribution = 0u32;
    for (count, matched, conf_sum) in metrics.calibration.values() {
        if *count > 0 {
            let hits_weighted = *matched * 10000;
            let conf_weighted = *conf_sum * 100;
            let gap = hits_weighted.abs_diff(conf_weighted);
            ece_contribution += gap / *count;
        }
    }
    let ece_bp = ece_contribution / u32::try_from(non_adv.len().max(1)).unwrap_or(1);

    // Render report
    let report = render_report(
        &metrics,
        &corpus,
        overall_accuracy_bp,
        ece_bp,
        u32::try_from(non_adv.len()).unwrap_or(0),
    );

    // Bless or verify
    let report_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/reports/extraction-eval.md"
    );
    if std::env::var("LEDGERS_EVAL_BLESS").is_ok() {
        std::fs::write(report_path, &report).expect("write report");
    } else if let Ok(existing) = std::fs::read_to_string(report_path) {
        assert_eq!(
            report, existing,
            "Report changed; regenerate with: LEDGERS_EVAL_BLESS=1 cargo test -p ledgers-extract --test eval recorded_set"
        );
    }

    // Check baselines
    let baseline = load_baseline();
    assert!(
        overall_accuracy_bp >= baseline.overall_accuracy,
        "Regression: {}bp < baseline {}bp",
        overall_accuracy_bp,
        baseline.overall_accuracy
    );

    for (field, tally) in &metrics.by_field {
        if let Some(&baseline_field_bp) = baseline.field_accuracy.get(field) {
            let field_acc_bp = (tally.hits * 10000) / tally.total;
            assert!(
                field_acc_bp >= baseline_field_bp,
                "Field {field} regression: {field_acc_bp}bp < baseline {baseline_field_bp}bp"
            );
        }
    }

    // Verify history
    if let Ok(history_json) = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/eval/history.json"
    )) && let Ok(history) = serde_json::from_str::<serde_json::Value>(&history_json)
        && let Some(entries) = history.get("entries").and_then(|e| e.as_array())
        && let Some(latest) = entries.last()
    {
        let hist_acc = u32::try_from(
            latest
                .get("overall_accuracy_bp")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
        )
        .unwrap_or(0);
        let hist_ece = u32::try_from(
            latest
                .get("ece_bp")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
        )
        .unwrap_or(0);
        let hist_cases = u32::try_from(
            latest
                .get("case_count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
        )
        .unwrap_or(0);

        assert_eq!(
            hist_acc, overall_accuracy_bp,
            "History: expected {overall_accuracy_bp}bp, got {hist_acc}bp; append a new row to history.json"
        );
        assert_eq!(
            hist_ece, ece_bp,
            "History ECE mismatch: expected {ece_bp}bp, got {hist_ece}bp"
        );
        assert_eq!(
            hist_cases,
            u32::try_from(non_adv.len()).unwrap_or(0),
            "History case count mismatch"
        );
    }
}

#[derive(Serialize, Deserialize)]
struct Baseline {
    #[serde(rename = "overall_accuracy_bp")]
    overall_accuracy: u32,
    #[serde(rename = "ece_bp")]
    ece: u32,
    #[serde(rename = "field_accuracy_bp")]
    field_accuracy: BTreeMap<String, u32>,
}

fn load_baseline() -> Baseline {
    serde_json::from_str(include_str!("eval/baseline.json")).expect("valid baseline JSON")
}

fn get_extracted_value(invoice: &ExtractedInvoice, field: &str) -> Option<String> {
    match field {
        "vendor_name" => invoice.vendor_name.clone(),
        "vendor_vat_id" => invoice.vendor_vat_id.clone(),
        "vendor_iban" => invoice.vendor_iban.clone(),
        "invoice_number" => invoice.invoice_number.clone(),
        "invoice_date" => invoice.invoice_date.clone(),
        "due_date" => invoice.due_date.clone(),
        "currency" => invoice.currency.clone(),
        "net_minor" => invoice.net_minor.map(|n| n.to_string()),
        "vat_minor" => invoice.vat_minor.map(|n| n.to_string()),
        "gross_minor" => invoice.gross_minor.map(|n| n.to_string()),
        _ => None,
    }
}

#[tokio::test]
async fn adversarial_tests() {
    let corpus = load_corpus();

    for case in corpus.iter().filter(|c| c.adversarial.is_some()) {
        let adv = case.adversarial.as_ref().unwrap();

        // Test: guard screening (checks for instruction-like phrases)
        if let Some(expected_flags) = &adv.guard_flags_expected {
            let flags = ledgers_extract::guard::screen(&case.text);
            if expected_flags.is_empty() {
                assert!(
                    flags.is_empty(),
                    "Case {}: expected no guard flags, got {:?}",
                    case.id,
                    flags
                );
            } else {
                for exp in expected_flags {
                    assert!(
                        flags.contains(&exp.as_str()),
                        "Case {}: missing guard flag '{}'",
                        case.id,
                        exp
                    );
                }
            }
        }

        // Test: PDF without text layer → NeedsPhoto
        if adv.expect_error == Some("NeedsPhoto".to_string()) {
            let pdf = ledgers_extract::fixture::pdf_with_text(&[""]);
            let doc = DocumentInput {
                bytes: &pdf,
                mime: "application/pdf",
                text_layer: None,
            };

            let model = ReplayModel {
                case_id: case.id.clone(),
                fast_reply: case.recorded.fast.clone(),
                strong_reply: case.recorded.strong.clone(),
                recorded_prompts: Arc::new(std::sync::Mutex::new(Vec::new())),
            };

            let extractor = HarnessExtractor::new(Arc::new(model), Reader::Anthropic);
            let result = extractor.extract(&doc).await;

            assert!(
                matches!(result, Err(ledgers_extract::ExtractError::NeedsPhoto)),
                "Case {}: expected NeedsPhoto",
                case.id
            );
        }

        // Test: fields expectations (for cases like adv_white_text_iban)
        if let Some(expected_fields) = &adv.fields_expected {
            let pdf = ledgers_extract::fixture::pdf_with_text(&[&case.text]);
            let doc = DocumentInput {
                bytes: &pdf,
                mime: "application/pdf",
                text_layer: Some(&case.text),
            };

            let model = ReplayModel {
                case_id: case.id.clone(),
                fast_reply: case.recorded.fast.clone(),
                strong_reply: case.recorded.strong.clone(),
                recorded_prompts: Arc::new(std::sync::Mutex::new(Vec::new())),
            };

            let extractor = HarnessExtractor::new(Arc::new(model), Reader::Anthropic);
            match extractor.extract(&doc).await {
                Ok(extraction) => {
                    for (field, expected_val) in expected_fields {
                        let got = get_extracted_value(&extraction.invoice, field);
                        let matched = match (expected_val, &got) {
                            (serde_json::Value::Null, None) => true,
                            (serde_json::Value::String(e), Some(v)) => normalize_string(e) == *v,
                            (serde_json::Value::Number(e), Some(v)) => {
                                e.as_i64() == v.trim().parse::<i64>().ok()
                            }
                            _ => false,
                        };
                        assert!(
                            matched,
                            "Case {}: field {} expected {:?}, got {:?}",
                            case.id, field, expected_val, got
                        );
                    }
                }
                Err(e) => {
                    panic!("Case {}: unexpected extraction error: {:?}", case.id, e);
                }
            }
        }
    }
}
