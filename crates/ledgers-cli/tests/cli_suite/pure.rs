//! Pure functions: registry checks, response mapping, and human formatting.

use super::common::descriptor;
use ledgers_cli::app::build_cli_registry;
use ledgers_cli::http::{CallOutcome, map_response};
use ledgers_cli::output::format_human;
use reqwest::StatusCode;
use serde_json::json;

#[test]
fn two_commands_with_the_same_cli_path_are_rejected() {
    let err = build_cli_registry(vec![
        descriptor(
            "reports.trial_balance",
            "GET",
            "/v1/reports/trial-balance",
            false,
            json!({}),
        ),
        descriptor(
            "reports.trial-balance",
            "GET",
            "/v1/reports/x",
            false,
            json!({}),
        ),
    ])
    .err()
    .expect("collision");
    assert!(err.contains("reports trial-balance"), "{err}");
}

#[test]
fn a_command_that_is_also_a_group_is_rejected() {
    let err = build_cli_registry(vec![
        descriptor("bills.match", "POST", "/v1/bills/match", false, json!({})),
        descriptor(
            "bills.match.deep",
            "POST",
            "/v1/bills/match/deep",
            false,
            json!({}),
        ),
    ])
    .err()
    .expect("prefix conflict");
    assert!(err.contains("both a command and a group"), "{err}");
}

#[test]
fn a_group_named_like_a_builtin_is_rejected() {
    let err = build_cli_registry(vec![descriptor(
        "serve.now",
        "GET",
        "/v1/serve/now",
        false,
        json!({}),
    )])
    .err()
    .expect("shadows serve");
    assert!(err.contains("shadows a built-in"), "{err}");
}

#[test]
fn duplicate_registration_is_rejected() {
    let err = build_cli_registry(vec![
        descriptor("bills.match", "POST", "/v1/bills/match", false, json!({})),
        descriptor("bills.match", "POST", "/v1/bills/match", false, json!({})),
    ])
    .err()
    .expect("duplicate");
    assert!(err.contains("already registered"), "{err}");
}

#[test]
fn approval_response_maps_to_needs_approval() {
    let body = json!({"status": "needs-approval", "approval": {
        "approval_id": "ap_1", "url": "https://x/ap_1", "reason": "over limit", "preview": {"lines": []}
    }});
    let outcome = map_response(StatusCode::OK, body, false).expect("maps");
    assert!(
        matches!(outcome, CallOutcome::NeedsApproval { ref url, .. } if url == "https://x/ap_1")
    );
}

#[test]
fn approval_without_a_url_is_a_malformed_response() {
    let body = json!({"status": "needs-approval", "approval": {"approval_id": "ap_1"}});
    assert!(map_response(StatusCode::OK, body, false).is_err());
}

#[test]
fn success_is_done_or_dry_run_depending_on_the_request() {
    let body = json!({"ok": true});
    assert_eq!(
        map_response(StatusCode::OK, body.clone(), false).expect("maps"),
        CallOutcome::Done(body.clone())
    );
    assert_eq!(
        map_response(StatusCode::OK, body.clone(), true).expect("maps"),
        CallOutcome::DryRun(body)
    );
}

#[test]
fn a_403_is_a_denial_with_the_server_reason() {
    let body = json!({"type": "https://ledgers.sh/problems/human-only", "detail": "this command is human-only"});
    assert_eq!(
        map_response(StatusCode::FORBIDDEN, body, false).expect("maps"),
        CallOutcome::Denied("this command is human-only".to_string())
    );
}

#[test]
fn other_error_statuses_are_errors_with_the_slug_when_present() {
    let body = json!({"type": "https://ledgers.sh/problems/invalid-input", "detail": "bad amount"});
    let err = map_response(StatusCode::BAD_REQUEST, body, false).expect_err("error");
    assert_eq!(err.to_string(), "invalid-input: bad amount");
}

#[test]
fn a_non_problem_error_body_is_kept_as_http_error() {
    let err = map_response(StatusCode::BAD_GATEWAY, json!("upstream"), false).expect_err("error");
    assert!(err.to_string().starts_with("HTTP 502"), "{err}");
}

#[test]
fn human_format_is_one_field_per_line() {
    assert_eq!(
        format_human(
            &json!({"entry_number": "E-1", "amount": 1250, "posted": true, "tags": ["a"]})
        ),
        "entry_number: E-1\namount: 1250\nposted: true\ntags: [\"a\"]"
    );
    assert_eq!(
        format_human(&json!([{"id": 1}, "two"])),
        "- {\"id\":1}\n- two"
    );
    assert_eq!(format_human(&json!([])), "(none)");
    assert_eq!(format_human(&json!({})), "(empty)");
}
