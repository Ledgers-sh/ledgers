//! The test vectors in docs/audit-format.md, pinned twice: computed
//! here against literals, and checked back against the document with
//! `include_str!`. Either alone can rot — a vector never pasted into
//! the document, or a hash in the document that drifted from the code,
//! would pass a one-sided check. The document is what a third party
//! implements against, so it must not disagree with the code.

use ledgers_audit::{Actor, AuditContext, AuditEvent, AuditRecord, Outcome};
use ledgers_audit::{canonical_json, event_hash, input_hash};
use ledgers_db::chain::GENESIS_HASH;
use serde_json::json;

const FORMAT_DOC: &str = include_str!("../../../docs/audit-format.md");

/// The invented company every vector belongs to (AGENTS.md: fixtures
/// are invented, never a real firm).
const COMPANY_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

/// Vector A — the genesis event: a person opens the company.
fn genesis_event() -> AuditEvent {
    let context = AuditContext::new(
        Actor::Person {
            person_id: concat!("usr_01HQZX9K3", "M7VN2P4R6T8W0", "Y1B3").to_owned(),
        },
        "req_01HQZX9K3M7VN2P4R6T8W0Y1B4",
        "01HQZX9K3M7VN2P4R6T8W0Y1B3D",
    );
    let record = AuditRecord::from_input(
        "company.open",
        &json!({"company": "Halden Works GmbH"}),
        Outcome::Allowed,
        vec![COMPANY_ID.to_owned()],
        None,
    );
    context.event(COMPANY_ID, 1, "2026-01-15T09:30:00Z", &record)
}

/// Vector B — seq 2: an agent key needs approval, and the reason
/// carries every escape the rule names plus a non-ASCII character.
fn agent_event() -> AuditEvent {
    let context = AuditContext::new(
        Actor::Agent {
            key_id: "key_01HQZX9K3M7VN2P4R6T8W0Y1B3".to_owned(),
            tier: "post-within-limits".to_owned(),
        },
        "req_01HQZX9K3M7VN2P4R6T8W0Y1B5",
        "01HQZX9K3M7VN2P4R6T8W0Y1B3E",
    )
    .with_approval("apr_01HQZX9K3M7VN2P4R6T8W0Y1B9");
    let record = AuditRecord::from_input(
        "journal.post_entry",
        &json!({"amount_minor": 125_000, "currency": "EUR"}),
        Outcome::NeedsApproval,
        vec![
            "01HQZX9K3M7VN2P4R6T8W0Y1B3F".to_owned(),
            "01HQZX9K3M7VN2P4R6T8W0Y1B3G".to_owned(),
        ],
        Some("above the daily limit for \"Müller GmbH\"; line 2\\3\nreview".to_owned()),
    );
    context.event(COMPANY_ID, 2, "2026-01-15T10:05:00Z", &record)
}

/// Vector C — the command input whose canonical bytes `input_hash` is
/// computed over.
fn nested_input() -> serde_json::Value {
    json!({
        "amount_minor": -12500,
        "currency": "EUR",
        "document": {"number": "INV-2026-0042", "supplier": "Müller GmbH"},
        "lines": [
            {"account": "1010", "minor": 12500},
            {"account": "1400", "minor": -12500}
        ]
    })
}

/// The canonical bytes of `event`: the chain's stored `payload` and the
/// bytes the hash covers.
fn canonical(event: &AuditEvent) -> String {
    canonical_json(&serde_json::to_value(event).expect("an event serialises"))
        .expect("an event is canonical")
}

#[test]
fn vector_a_the_genesis_event() {
    assert_eq!(GENESIS_HASH, "0");
    let event = genesis_event();
    assert_eq!(
        canonical(&event),
        // Sorted keys: actor, approval_id, at, command, company_id, id,
        // input_hash, objects, outcome, reason, request_id, seq.
        concat!(
            r#"{"actor":{"kind":"person","person_id":"usr_01HQZX9K3M7VN2P4R6T8W0Y1B3"},"#,
            r#""approval_id":null,"at":"2026-01-15T09:30:00Z","command":"company.open","#,
            r#""company_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV","id":"01HQZX9K3M7VN2P4R6T8W0Y1B3D","#,
            r#""input_hash":"INPUT_HASH_A","objects":["01ARZ3NDEKTSV4RRFFQ69G5FAV"],"#,
            r#""outcome":"allowed","reason":null,"request_id":"req_01HQZX9K3M7VN2P4R6T8W0Y1B4","#,
            r#""seq":1}"#
        )
        .replace("INPUT_HASH_A", &event.input_hash)
    );
    let hash = event_hash(GENESIS_HASH, &event).expect("hashed");
    assert_eq!(hash.len(), 64);
    assert!(
        hash.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );
    assert_doc_has(&canonical(&event), &hash);
}

#[test]
fn vector_b_the_agent_needing_approval_with_escapes() {
    let event = agent_event();
    let text = canonical(&event);
    // A quote, a backslash and a newline escaped; `ü` written
    // literally rather than as an escape.
    assert!(text.contains(r#"\"Müller GmbH\""#), "{text}");
    assert!(text.contains(r"line 2\\3\nreview"), "{text}");
    assert!(!text.contains("\\u00fc"), "non-ASCII must stay literal");
    assert!(text.contains(r#""tier":"post-within-limits""#), "{text}");
    assert!(text.contains(r#""outcome":"needs-approval""#), "{text}");
    assert!(
        text.contains(r#""approval_id":"apr_01HQZX9K3M7VN2P4R6T8W0Y1B9""#),
        "{text}"
    );

    // Linked onto vector A's hash, not onto genesis.
    let prev = event_hash(GENESIS_HASH, &genesis_event()).expect("hashed");
    let hash = event_hash(&prev, &event).expect("hashed");
    assert_ne!(hash, event_hash(GENESIS_HASH, &event).expect("hashed"));
    assert_doc_has(&text, &hash);
}

#[test]
fn vector_c_the_nested_command_input() {
    let input = nested_input();
    let text = canonical_json(&input).expect("canonical");
    // Nested objects sort at every depth, and the negative amount is a
    // plain integer — the ledger never sees a float.
    assert_eq!(
        text,
        concat!(
            r#"{"amount_minor":-12500,"currency":"EUR","document":{"number":"INV-2026-0042","#,
            r#""supplier":"Müller GmbH"},"lines":[{"account":"1010","minor":12500},"#,
            r#"{"account":"1400","minor":-12500}]}"#
        )
    );
    let hash = input_hash(&input);
    assert_eq!(hash.len(), 64);
    assert_doc_has(&text, &hash);
}

/// The document must carry the exact canonical bytes and the hash
/// computed from them, and the rules it names.
#[track_caller]
fn assert_doc_has(canonical_text: &str, hash: &str) {
    for topic in ["RFC 8785", "GENESIS_HASH", "audit.anchor", "append-only"] {
        assert!(
            FORMAT_DOC.contains(topic),
            "docs/audit-format.md should mention {topic}"
        );
    }
    assert!(
        FORMAT_DOC.contains(canonical_text),
        "docs/audit-format.md does not contain this vector's canonical JSON:\n{canonical_text}\n\
         Update the document (the vector is the specification a third party implements against)."
    );
    assert!(
        FORMAT_DOC.contains(hash),
        "docs/audit-format.md does not contain this vector's hash {hash}"
    );
}
