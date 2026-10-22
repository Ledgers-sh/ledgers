//! Output formatting for `--json` and human-readable modes.
//!
//! Where output goes:
//! - success, dry-run preview, and approval URL: stdout
//! - denial and errors in human mode: stderr
//! - `--json`: every outcome, including denials and errors, is one JSON object on stdout

use crate::error::CliError;
use crate::http::CallOutcome;
use serde_json::{Value, json};
use std::io::{self, Write};

/// Writes a command outcome.
///
/// # Errors
/// Returns an error if writing to `out` or `err` fails.
pub fn write_outcome(
    outcome: &CallOutcome,
    json: bool,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> io::Result<()> {
    match outcome {
        CallOutcome::Done(value) => {
            if json {
                writeln!(out, "{}", pretty(value))
            } else {
                writeln!(out, "{}", format_human(value))
            }
        }
        CallOutcome::DryRun(preview) => {
            if json {
                writeln!(out, "{}", pretty(preview))
            } else {
                writeln!(
                    out,
                    "Dry run (nothing was written):\n{}",
                    format_human(preview)
                )
            }
        }
        CallOutcome::NeedsApproval {
            approval_id,
            url,
            reason,
            preview,
        } => {
            if json {
                let body = json!({
                    "status": "needs-approval",
                    "approval": {
                        "approval_id": approval_id,
                        "url": url,
                        "reason": reason,
                        "preview": preview,
                    }
                });
                writeln!(out, "{}", pretty(&body))
            } else {
                writeln!(
                    out,
                    "Approval needed: {reason}\nApproval ID: {approval_id}\nApprove at: {url}"
                )
            }
        }
        CallOutcome::Denied(reason) => {
            if json {
                writeln!(
                    out,
                    "{}",
                    pretty(&json!({"status": "denied", "reason": reason}))
                )
            } else {
                writeln!(err, "denied: {reason}")
            }
        }
    }
}

/// Writes an error.
///
/// # Errors
/// Returns an error if writing to `out` or `err` fails.
pub fn write_error(
    error: &CliError,
    json: bool,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> io::Result<()> {
    if json {
        writeln!(
            out,
            "{}",
            pretty(&json!({"status": "error", "error": error.to_json()}))
        )
    } else {
        writeln!(err, "error: {error}")
    }
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

/// Renders JSON for people: one `key: value` line per object field, one `- item`
/// line per array element. Nested values are compact JSON.
#[must_use]
pub fn format_human(value: &Value) -> String {
    match value {
        Value::Object(map) if map.is_empty() => "(empty)".to_string(),
        Value::Object(map) => map
            .iter()
            .map(|(key, v)| format!("{key}: {}", inline(v)))
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Array(items) if items.is_empty() => "(none)".to_string(),
        Value::Array(items) => items
            .iter()
            .map(|v| format!("- {}", inline(v)))
            .collect::<Vec<_>>()
            .join("\n"),
        other => inline(other),
    }
}

fn inline(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}
