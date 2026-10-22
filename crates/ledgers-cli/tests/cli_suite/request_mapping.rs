//! Each registry command maps to the right method, path, query and body.

use super::common::{descriptor, registry_of};
use ledgers_cli::app::{build_root, find_descriptor, invocation, leaf_path};
use ledgers_cli::request::{Invocation, plan};
use ledgers_commands::registry::default_commands;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

fn params(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

fn input(value: &Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

/// The clap path for a command name, parsed from the CLI argv.
fn parse(
    registry: &ledgers_commands::Registry,
    argv: &[&str],
) -> (ledgers_commands::CommandDescriptor, Invocation) {
    let mut full = vec!["ledgers"];
    full.extend_from_slice(argv);
    let root = build_root(registry);
    let matches = root.try_get_matches_from(full).expect("argv parses");
    let (path, leaf) = leaf_path(&matches);
    let d = find_descriptor(registry, &path)
        .expect("command exists")
        .clone();
    let inv = invocation(&d, leaf).expect("invocation builds");
    (d, inv)
}

#[test]
fn entries_post_is_post_to_collection_with_input_as_body() {
    let registry = registry_of(default_commands());
    let d = registry
        .get("entries.post")
        .expect("entries.post registered");
    let inv = Invocation {
        input: input(&json!({"description": "Rent", "lines": []})),
        ..Invocation::default()
    };
    let req = plan(d, &inv).expect("plans");
    assert_eq!(req.method, "POST");
    assert_eq!(req.path, "/v1/entries");
    assert!(req.query.is_empty());
    assert_eq!(req.body, Some(json!({"description": "Rent", "lines": []})));
}

#[test]
fn entries_reverse_fills_external_id_in_path_and_body() {
    let registry = registry_of(default_commands());
    let d = registry
        .get("entries.reverse")
        .expect("entries.reverse registered");
    let inv = Invocation {
        path_params: params(&[("external_id", "EXT-42")]),
        input: input(&json!({"reason": "typo"})),
        ..Invocation::default()
    };
    let req = plan(d, &inv).expect("plans");
    assert_eq!(req.method, "POST");
    assert_eq!(req.path, "/v1/entries/EXT-42/reverse");
    assert_eq!(
        req.body,
        Some(json!({"reason": "typo", "external_id": "EXT-42"}))
    );
}

#[test]
fn entries_reverse_without_its_id_is_an_error() {
    let registry = registry_of(default_commands());
    let d = registry.get("entries.reverse").expect("registered");
    let err = plan(d, &Invocation::default()).expect_err("missing path id");
    assert!(err.to_string().contains("external_id"), "{err}");
}

#[test]
fn entries_list_is_get_with_no_body() {
    let registry = registry_of(default_commands());
    let d = registry.get("entries.list").expect("registered");
    let req = plan(d, &Invocation::default()).expect("plans");
    assert_eq!(req.method, "GET");
    assert_eq!(req.path, "/v1/entries");
    assert_eq!(req.body, None);
    assert!(req.query.is_empty());
}

#[test]
fn reports_map_to_their_hyphenated_routes() {
    let registry = registry_of(default_commands());
    let cases = [
        ("reports.trial_balance", "/v1/reports/trial-balance"),
        ("reports.profit_and_loss", "/v1/reports/p-and-l"),
        ("reports.balance_sheet", "/v1/reports/balance-sheet"),
        (
            "reports.account_balance_by_dimension",
            "/v1/reports/account-balance-by-dimension",
        ),
    ];
    for (name, path) in cases {
        let d = registry
            .get(name)
            .unwrap_or_else(|| panic!("{name} registered"));
        let req = plan(d, &Invocation::default()).expect("plans");
        assert_eq!(req.method, "GET", "{name}");
        assert_eq!(req.path, path, "{name}");
        assert_eq!(req.body, None, "{name}");
    }
}

#[test]
fn every_registry_command_maps_to_its_descriptor_method_and_path() {
    // Generic over the registry, so commands added later are covered without edits.
    let registry = registry_of(default_commands());
    assert!(registry.iter().count() > 0);
    for d in registry.iter() {
        let mut inv = Invocation::default();
        for placeholder in ledgers_cli::request::path_param_names(&d.http_path) {
            inv.path_params.insert(placeholder, "ID-1".to_string());
        }
        let req = plan(d, &inv).unwrap_or_else(|e| panic!("{}: {e}", d.name));
        assert_eq!(req.method, d.http_method, "{}", d.name);
        let mut expected = d.http_path.clone();
        for placeholder in ledgers_cli::request::path_param_names(&d.http_path) {
            expected = expected.replace(&format!("{{{placeholder}}}"), "ID-1");
        }
        assert_eq!(req.path, expected, "{}", d.name);
        assert_eq!(req.body.is_some(), d.http_method != "GET", "{}", d.name);
    }
}

#[test]
fn clap_argv_maps_to_the_request_for_a_reverse() {
    let registry = registry_of(default_commands());
    let (d, inv) = parse(
        &registry,
        &[
            "entries",
            "reverse",
            "EXT-9",
            "--input",
            r#"{"reason":"dup"}"#,
        ],
    );
    let req = plan(&d, &inv).expect("plans");
    assert_eq!(req.path, "/v1/entries/EXT-9/reverse");
    assert_eq!(
        req.body,
        Some(json!({"reason": "dup", "external_id": "EXT-9"}))
    );
}

#[test]
fn schema_flags_become_input_fields_with_their_types() {
    let schema = json!({
        "type": "object",
        "required": ["bill_id"],
        "properties": {
            "bill_id": {"type": "string"},
            "amount_minor": {"type": "integer"},
            "auto_post": {"type": "boolean"},
            "tags": {"type": "array", "items": {"type": "string"}}
        }
    });
    let registry = registry_of(vec![descriptor(
        "bills.match",
        "POST",
        "/v1/bills/match",
        false,
        schema,
    )]);
    let (d, inv) = parse(
        &registry,
        &[
            "bills",
            "match",
            "--bill-id",
            "B-1",
            "--amount-minor",
            "12500",
            "--auto-post",
            "--tags",
            r#"["a","b"]"#,
        ],
    );
    let req = plan(&d, &inv).expect("plans");
    assert_eq!(req.path, "/v1/bills/match");
    assert_eq!(
        req.body,
        Some(
            json!({"bill_id": "B-1", "amount_minor": 12500, "auto_post": true, "tags": ["a", "b"]})
        )
    );
}

#[test]
fn missing_required_schema_field_is_rejected_before_sending() {
    let schema = json!({"type": "object", "required": ["bill_id"], "properties": {"bill_id": {"type": "string"}}});
    let registry = registry_of(vec![descriptor(
        "bills.match",
        "POST",
        "/v1/bills/match",
        false,
        schema,
    )]);
    let (d, inv) = parse(&registry, &["bills", "match"]);
    let err = plan(&d, &inv).expect_err("required field missing");
    assert!(err.to_string().contains("--bill-id"), "{err}");
}

#[test]
fn a_bad_integer_flag_is_an_input_error() {
    let schema = json!({"type": "object", "properties": {"amount_minor": {"type": "integer"}}});
    let registry = registry_of(vec![descriptor(
        "bills.match",
        "POST",
        "/v1/bills/match",
        false,
        schema,
    )]);
    let root = build_root(&registry);
    let matches = root
        .try_get_matches_from(["ledgers", "bills", "match", "--amount-minor", "twelve"])
        .expect("argv parses");
    let (path, leaf) = leaf_path(&matches);
    let d = find_descriptor(&registry, &path).expect("exists");
    assert!(invocation(d, leaf).is_err());
}

#[test]
fn get_command_sends_scalar_input_as_query_parameters() {
    let schema = json!({"type": "object", "properties": {"account": {"type": "string"}, "limit": {"type": "integer"}}});
    let registry = registry_of(vec![descriptor(
        "entries.list",
        "GET",
        "/v1/entries",
        false,
        schema,
    )]);
    let (d, inv) = parse(
        &registry,
        &["entries", "list", "--account", "1100", "--limit", "5"],
    );
    let req = plan(&d, &inv).expect("plans");
    assert_eq!(req.body, None);
    let mut query = req.query.clone();
    query.sort();
    assert_eq!(
        query,
        vec![
            ("account".to_string(), "1100".to_string()),
            ("limit".to_string(), "5".to_string())
        ]
    );
}

#[test]
fn dry_run_is_sent_only_when_asked_and_only_for_supporting_commands() {
    let registry = registry_of(vec![descriptor(
        "bills.match",
        "POST",
        "/v1/bills/match",
        true,
        json!({}),
    )]);
    let (d, inv) = parse(&registry, &["bills", "match", "--dry-run"]);
    let req = plan(&d, &inv).expect("plans");
    assert_eq!(req.body, Some(json!({"dry_run": true})));

    let (_, plain) = parse(&registry, &["bills", "match"]);
    assert_eq!(plan(&d, &plain).expect("plans").body, Some(json!({})));

    // A command without dry-run support has no --dry-run flag at all.
    let no_dry = registry_of(default_commands());
    let root = build_root(&no_dry);
    assert!(
        root.try_get_matches_from(["ledgers", "entries", "post", "--dry-run"])
            .is_err()
    );
}

#[test]
fn dry_run_get_adds_query_flag() {
    let registry = registry_of(vec![descriptor(
        "reports.preview",
        "GET",
        "/v1/reports/preview",
        true,
        json!({}),
    )]);
    let (d, inv) = parse(&registry, &["reports", "preview", "--dry-run"]);
    let req = plan(&d, &inv).expect("plans");
    assert_eq!(req.query, vec![("dry_run".to_string(), "true".to_string())]);
}

#[test]
fn path_values_are_percent_encoded() {
    let registry = registry_of(vec![descriptor(
        "entries.reverse",
        "POST",
        "/v1/entries/{external_id}/reverse",
        false,
        json!({}),
    )]);
    let (d, inv) = parse(&registry, &["entries", "reverse", "a/b c"]);
    let req = plan(&d, &inv).expect("plans");
    assert_eq!(req.path, "/v1/entries/a%2Fb%20c/reverse");
}
