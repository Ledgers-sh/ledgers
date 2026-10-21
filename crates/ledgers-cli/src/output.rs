//! Output formatting: human-readable and JSON.

use crate::CallOutcome;
use serde_json::Value;

/// Formats command output for human or machine consumption.
pub fn format_outcome(outcome: &CallOutcome, json: bool) -> String {
    match outcome {
        CallOutcome::Done(value) => {
            if json {
                serde_json::to_string_pretty(value).unwrap_or_default()
            } else {
                format_human(value)
            }
        }
        CallOutcome::DryRun(preview) => {
            if json {
                serde_json::to_string_pretty(preview).unwrap_or_default()
            } else {
                format!("Dry-run preview:\n{}", format_human(preview))
            }
        }
        CallOutcome::NeedsApproval {
            url,
            approval_id,
            reason,
            preview,
        } => {
            if json {
                serde_json::json!({
                    "needsApproval": {
                        "approval_id": approval_id,
                        "url": url,
                        "reason": reason,
                        "preview": preview
                    }
                })
                .to_string()
            } else {
                format!(
                    "needs approval: {}\n\nReason: {}\n\nView/approve at:\n{}",
                    approval_id, reason, url
                )
            }
        }
        CallOutcome::Denied(reason) => {
            if json {
                serde_json::json!({
                    "denied": reason
                })
                .to_string()
            } else {
                format!("denied: {}", reason)
            }
        }
    }
}

/// Simple human-readable formatting of JSON.
fn format_human(value: &Value) -> String {
    match value {
        Value::Object(obj) => {
            let items: Vec<String> = obj
                .iter()
                .map(|(k, v)| format!("  {}: {}", k, format_human(v)))
                .collect();
            format!("{{\n{}\n}}", items.join(",\n"))
        }
        Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(format_human).collect();
            format!("[\n  {}\n]", items.join(",\n  "))
        }
        Value::String(s) => format!("\"{}\"", s.escape_default()),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
    }
}
