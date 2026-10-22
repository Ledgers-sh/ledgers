//! Exit codes and `--json` output against a local mock server.
//!
//! 0 ok, 1 error, 2 needs approval (URL printed), 3 denied (HTTP 403).

use super::common::{context_for, descriptor, registry_of, run_cli};
use ledgers_cli::ExitCode;
use ledgers_commands::registry::default_commands;
use serde_json::{Value, json};

const APPROVAL_URL: &str = "https://ledgers.example/approvals/ap_77";

fn approval_body() -> Value {
    json!({
        "status": "needs-approval",
        "approval": {
            "approval_id": "ap_77",
            "url": APPROVAL_URL,
            "reason": "amount is above the agent limit",
            "preview": {"lines": [], "totals": {"EUR": "1200.00"}}
        }
    })
}

#[tokio::test]
async fn success_exits_0_and_json_is_the_response_body() {
    let mut server = mockito::Server::new_async().await;
    let body = json!({"entry_number": "E-100", "status": "posted"});
    let mock = server
        .mock("POST", "/v1/entries")
        .match_header("authorization", "Bearer test-key")
        .match_body(mockito::Matcher::Json(json!({"description": "Rent"})))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body.to_string())
        .create_async()
        .await;

    let registry = registry_of(default_commands());
    let (code, out, err) = run_cli(
        &registry,
        &context_for(&server.url()),
        &[
            "entries",
            "post",
            "--input",
            r#"{"description":"Rent"}"#,
            "--json",
        ],
    )
    .await;

    mock.assert_async().await;
    assert_eq!(code, ExitCode::Ok, "stderr: {err}");
    let printed: Value = serde_json::from_str(&out).expect("stdout is JSON");
    assert_eq!(printed, body);
    assert!(err.is_empty());
}

#[tokio::test]
async fn human_output_is_readable_not_json() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("POST", "/v1/entries")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(json!({"entry_number": "E-100"}).to_string())
        .create_async()
        .await;

    let registry = registry_of(default_commands());
    let (code, out, _err) = run_cli(
        &registry,
        &context_for(&server.url()),
        &["entries", "post", "--input", "{}"],
    )
    .await;

    assert_eq!(code, ExitCode::Ok);
    assert_eq!(out.trim(), "entry_number: E-100");
}

#[tokio::test]
async fn needs_approval_exits_2_and_prints_the_approval_url() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("POST", "/v1/entries")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(approval_body().to_string())
        .create_async()
        .await;

    let registry = registry_of(default_commands());
    let ctx = context_for(&server.url());
    let (code, out, err) = run_cli(&registry, &ctx, &["entries", "post", "--input", "{}"]).await;

    assert_eq!(code, ExitCode::NeedsApproval);
    assert!(out.contains(APPROVAL_URL), "stdout: {out}");
    assert!(out.contains("ap_77"), "stdout: {out}");
    assert!(err.is_empty(), "stderr: {err}");

    let (code, out, _) = run_cli(
        &registry,
        &ctx,
        &["entries", "post", "--input", "{}", "--json"],
    )
    .await;
    assert_eq!(code, ExitCode::NeedsApproval);
    let printed: Value = serde_json::from_str(&out).expect("JSON");
    assert_eq!(printed["status"], "needs-approval");
    assert_eq!(printed["approval"]["url"], APPROVAL_URL);
    assert_eq!(printed["approval"]["approval_id"], "ap_77");
    assert_eq!(printed["approval"]["preview"]["totals"]["EUR"], "1200.00");
}

#[tokio::test]
async fn policy_denial_exits_3() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("POST", "/v1/entries")
        .with_status(403)
        .with_header("content-type", "application/problem+json")
        .with_body(
            json!({
                "type": "https://ledgers.sh/problems/policy-denied",
                "title": "Policy denied",
                "status": 403,
                "detail": "policy decision is to deny"
            })
            .to_string(),
        )
        .create_async()
        .await;

    let registry = registry_of(default_commands());
    let ctx = context_for(&server.url());
    let (code, out, err) = run_cli(&registry, &ctx, &["entries", "post", "--input", "{}"]).await;
    assert_eq!(code, ExitCode::PolicyDenied);
    assert!(
        err.contains("denied: policy decision is to deny"),
        "stderr: {err}"
    );
    assert!(out.is_empty(), "stdout: {out}");

    let (code, out, _) = run_cli(
        &registry,
        &ctx,
        &["entries", "post", "--input", "{}", "--json"],
    )
    .await;
    assert_eq!(code, ExitCode::PolicyDenied);
    let printed: Value = serde_json::from_str(&out).expect("JSON");
    assert_eq!(printed["status"], "denied");
    assert_eq!(printed["reason"], "policy decision is to deny");
}

#[tokio::test]
async fn server_error_exits_1_with_the_problem_slug() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("POST", "/v1/entries")
        .with_status(500)
        .with_header("content-type", "application/problem+json")
        .with_body(
            json!({
                "type": "https://ledgers.sh/problems/database-error",
                "status": 500,
                "detail": "disk on fire"
            })
            .to_string(),
        )
        .create_async()
        .await;

    let registry = registry_of(default_commands());
    let ctx = context_for(&server.url());
    let (code, _out, err) = run_cli(&registry, &ctx, &["entries", "post", "--input", "{}"]).await;
    assert_eq!(code, ExitCode::Error);
    assert!(err.starts_with("error: "), "stderr: {err}");
    assert!(
        err.contains("database-error: disk on fire"),
        "stderr: {err}"
    );

    let (code, out, _) = run_cli(
        &registry,
        &ctx,
        &["entries", "post", "--input", "{}", "--json"],
    )
    .await;
    assert_eq!(code, ExitCode::Error);
    let printed: Value = serde_json::from_str(&out).expect("JSON");
    assert_eq!(printed["status"], "error");
    assert_eq!(printed["error"]["slug"], "database-error");
    assert_eq!(printed["error"]["status"], 500);
}

#[tokio::test]
async fn unreachable_server_exits_1() {
    let registry = registry_of(default_commands());
    // Port 9 (discard) on loopback is not listening; the connection is refused.
    let (code, _out, err) = run_cli(
        &registry,
        &context_for("http://127.0.0.1:9"),
        &["entries", "list"],
    )
    .await;
    assert_eq!(code, ExitCode::Error);
    assert!(err.contains("network error"), "stderr: {err}");
}

#[tokio::test]
async fn dry_run_posts_the_flag_and_exits_0() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/v1/bills/match")
        .match_body(mockito::Matcher::Json(
            json!({"bill_id": "B-1", "dry_run": true}),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(json!({"matched": true}).to_string())
        .create_async()
        .await;

    let schema = json!({"type": "object", "properties": {"bill_id": {"type": "string"}}});
    let registry = registry_of(vec![descriptor(
        "bills.match",
        "POST",
        "/v1/bills/match",
        true,
        schema,
    )]);
    let (code, out, _err) = run_cli(
        &registry,
        &context_for(&server.url()),
        &["bills", "match", "--bill-id", "B-1", "--dry-run"],
    )
    .await;

    mock.assert_async().await;
    assert_eq!(code, ExitCode::Ok);
    assert!(
        out.starts_with("Dry run (nothing was written):"),
        "stdout: {out}"
    );
    assert!(out.contains("matched: true"), "stdout: {out}");
}

#[tokio::test]
async fn path_argument_is_sent_in_the_url() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/v1/entries/EXT-9/reverse")
        .match_header("authorization", "Bearer test-key")
        .match_body(mockito::Matcher::Json(
            json!({"reason": "dup", "external_id": "EXT-9"}),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(json!({"reversed": "EXT-9"}).to_string())
        .create_async()
        .await;

    let registry = registry_of(default_commands());
    let (code, _out, err) = run_cli(
        &registry,
        &context_for(&server.url()),
        &[
            "entries",
            "reverse",
            "EXT-9",
            "--input",
            r#"{"reason":"dup"}"#,
        ],
    )
    .await;
    mock.assert_async().await;
    assert_eq!(code, ExitCode::Ok, "stderr: {err}");
}

#[tokio::test]
async fn get_query_parameters_reach_the_server() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/v1/entries")
        .match_query(mockito::Matcher::UrlEncoded(
            "account".into(),
            "1100".into(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(json!([]).to_string())
        .create_async()
        .await;

    let schema = json!({"type": "object", "properties": {"account": {"type": "string"}}});
    let registry = registry_of(vec![descriptor(
        "entries.list",
        "GET",
        "/v1/entries",
        false,
        schema,
    )]);
    let (code, out, _err) = run_cli(
        &registry,
        &context_for(&server.url()),
        &["entries", "list", "--account", "1100", "--json"],
    )
    .await;
    mock.assert_async().await;
    assert_eq!(code, ExitCode::Ok);
    assert_eq!(
        serde_json::from_str::<Value>(&out).expect("JSON"),
        json!([])
    );
}

#[tokio::test]
async fn invalid_input_exits_1_without_calling_the_server() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/v1/entries")
        .expect(0)
        .create_async()
        .await;

    let registry = registry_of(default_commands());
    let (code, _out, err) = run_cli(
        &registry,
        &context_for(&server.url()),
        &["entries", "post", "--input", "[1,2]"],
    )
    .await;
    mock.assert_async().await;
    assert_eq!(code, ExitCode::Error);
    assert!(err.contains("must be a JSON object"), "stderr: {err}");
}

#[tokio::test]
async fn help_and_usage_errors_use_clap_exit_codes() {
    let registry = registry_of(default_commands());
    let ctx = context_for("http://127.0.0.1:9");

    let (code, out, _) = run_cli(&registry, &ctx, &["entries", "--help"]).await;
    assert_eq!(code, ExitCode::Ok);
    assert!(out.contains("Usage: ledgers entries"), "stdout: {out}");

    let (code, _out, err) = run_cli(&registry, &ctx, &["entries", "reverse"]).await;
    assert_eq!(code, ExitCode::Error);
    assert!(err.contains("EXTERNAL-ID"), "stderr: {err}");
}
