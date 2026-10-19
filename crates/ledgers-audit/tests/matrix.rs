//! The audit scenario matrix over every backing engine the environment
//! provides — in-memory SQLite always, a throwaway Postgres 16 database
//! when `FZ_TEST_POSTGRES_URL` names one. One test definition, every
//! dialect, the same shape as `ledgers-db`'s matrices.
//!
//! What it proves per dialect: the chain appends, exports and verifies;
//! `verify_db` agrees with the exported bytes; the append-only triggers
//! refuse both an `UPDATE` and a `DELETE`; a fork is a unique violation
//! rather than a stored disagreement; and an anchor lands in the outbox
//! at the configured interval with the head's hash as its payload.

use cratefield_core::{Database, Statement};
use cratefield_testing::TestHarness;
use ledgers_audit::{
    ANCHOR_TOPIC, Actor, AuditConfig, AuditContext, AuditRecord, Outcome, anchor_statement,
    append_statements, export, record, verify, verify_db,
};
use ledgers_db::retry::RetryPolicy;
use ledgers_db::{DbModule, chain};
use serde_json::json;

/// The invented company the scenarios audit for (AGENTS.md: fixtures
/// are invented, never a real firm).
const COMPANY_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const AT: &str = "2026-01-15T09:30:00Z";

/// The guard module: this crate writes to `audit_event`, which
/// `ledgers-db` owns, so it is the module whose migrations the chain
/// and its triggers live in.
fn modules() -> Vec<Box<dyn cratefield_core::Module>> {
    vec![Box::new(DbModule)]
}

fn context(index: usize) -> AuditContext {
    AuditContext::new(
        Actor::Person {
            person_id: concat!("usr_01HQZX9K3", "M7VN2P4R6T8W0", "Y1B3").to_owned(),
        },
        format!("req_01HQZX9K3M7VN2P4R6T8W0Y1B{index}"),
        format!("01HQZX9K3M7VN2P4R6T8W0Y1B3{index}"),
    )
}

fn audit_record(index: usize) -> AuditRecord {
    AuditRecord::from_input(
        "journal.post_entry",
        &json!({"n": index}),
        Outcome::Allowed,
        vec![format!("entry-{index}")],
        None,
    )
}

/// Appends one event as its own batch, the way a command with no other
/// change would, and returns the hash it wrote.
async fn append_one(db: &dyn Database, index: usize, config: &AuditConfig) -> String {
    record(
        db,
        config,
        COMPANY_ID,
        &context(index),
        &audit_record(index),
        AT,
        RetryPolicy::default(),
        |_| {},
    )
    .await
    .unwrap_or_else(|err| panic!("append event {index}: {err}"))
    .hash
}

#[test]
fn the_audit_chain_on_every_available_dialect() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(scenarios(&*kit.db));
    }
}

async fn scenarios(db: &dyn Database) {
    let config = AuditConfig::default();

    // 1. Three events, each in its own batch, and the head is where the
    //    last append left it.
    let mut hashes = Vec::new();
    for index in 0..3 {
        hashes.push(append_one(db, index, &config).await);
    }
    let head = chain::read_head(db, COMPANY_ID)
        .await
        .expect("read the head");
    assert_eq!(head.seq, 3);
    assert_eq!(head.hash, hashes[2]);

    // 2. Export, then verify the bytes; then verify in one step. Both
    //    must agree — `verify_db` is the export plus the verifier, and a
    //    disagreement would mean the file and the table tell different
    //    stories. Three lines, so four newline-separated pieces: the
    //    last terminator leaves one empty tail.
    let bytes = export(db, COMPANY_ID).await.expect("export the chain");
    let verified = verify(&bytes).expect("the export verifies");
    assert_eq!((verified.events, verified.head_seq), (3, 3));
    assert_eq!(verified.head_hash, head.hash);
    assert_eq!(bytes.split(|byte| *byte == b'\n').count(), 4);
    assert_eq!(
        verify_db(db, COMPANY_ID).await.expect("verify_db"),
        verified
    );

    // 3. The chain is append-only: the database refuses an update, delete,
    //    and (on Postgres) truncate, not just the library.
    assert_refused(
        db,
        "UPDATE audit_event SET hash = 'rewritten' WHERE company_id = ? AND seq = 1",
    )
    .await;
    assert_refused(
        db,
        "DELETE FROM audit_event WHERE company_id = ? AND seq = 1",
    )
    .await;
    // TRUNCATE is Postgres-specific; SQLite doesn't have the same support.
    // Detect Postgres by trying a Postgres-specific query.
    if is_postgres(db).await {
        assert_refused(db, "TRUNCATE TABLE audit_event").await;
    }
    assert_eq!(
        chain::read_head(db, COMPANY_ID).await.expect("head").hash,
        head.hash,
        "intact after the two refusals"
    );

    // 4. A fork is a unique violation, never a stored disagreement: two
    //    appends planned against the *same* head both claim seq 4. The
    //    head is read once and copied, as two concurrent writers would
    //    each hold it.
    let read = chain::read_head(db, COMPANY_ID).await.expect("head");
    let writer_a = append_statements(
        COMPANY_ID,
        &read,
        &context(9),
        &audit_record(9),
        AT,
        &config,
    )
    .expect("planned");
    let writer_b = append_statements(
        COMPANY_ID,
        &read,
        &context(8),
        &audit_record(8),
        AT,
        &config,
    )
    .expect("planned");
    assert_eq!(
        (writer_a.seq, writer_b.seq),
        (4, 4),
        "both writers planned against the head they both read"
    );
    db.batch_atomic(&writer_a.statements)
        .await
        .expect("the first of two racing writers commits");
    let raced = db.batch_atomic(&writer_b.statements).await;
    let err = raced.expect_err("the loser must not store a second seq 4");
    assert!(
        ledgers_db::retry::is_unique_violation(&err),
        "the fork must surface as a unique violation, the one class retry replans: {err}"
    );
    let head_after = chain::read_head(db, COMPANY_ID).await.expect("head");
    assert_eq!((head_after.seq, head_after.hash), (4, writer_a.hash));
    let bytes = export(db, COMPANY_ID).await.expect("export");
    assert_eq!(verify(&bytes).expect("the chain still verifies").events, 4);
    assert!(
        !String::from_utf8_lossy(&bytes).contains(&writer_b.event.id),
        "the losing writer's event must be absent from the chain"
    );

    // 5. An anchor lands at the configured interval, in the outbox.
    anchors_at_every_second(db).await;
}

/// `anchor_every` 2, so the second append publishes and the first does not.
async fn anchors_at_every_second(db: &dyn Database) {
    let company = "01ANCHORCOMPANY00000000000A";
    let config = AuditConfig {
        anchor_every: 2,
        ..AuditConfig::default()
    };
    let first = record(
        db,
        &config,
        company,
        &context(1),
        &audit_record(1),
        AT,
        RetryPolicy::default(),
        |_| {},
    )
    .await
    .expect("append seq 1");
    assert_eq!(first.seq, 1);
    assert_eq!(
        anchors(db, company).await,
        0,
        "seq 1 is not a multiple of anchor_every, so nothing is published"
    );

    let second = record(
        db,
        &config,
        company,
        &context(2),
        &audit_record(2),
        AT,
        RetryPolicy::default(),
        |_| {},
    )
    .await
    .expect("append seq 2");
    assert_eq!(second.seq, 2);
    assert_eq!(second.statements.len(), 2, "append plus anchor enqueue");
    assert_eq!(anchors(db, company).await, 1, "seq 2 is an anchor point");
    assert_eq!(
        anchor_rows(db, company).await,
        vec![(
            ANCHOR_TOPIC.to_owned(),
            format!(
                r#"{{"company_id":"{company}","hash":"{}","seq":2}}"#,
                second.hash
            ),
        )],
        "the anchor publishes the head it was appended with"
    );
    // The id is derived from the event id, so a retried batch takes the
    // same primary key and cannot enqueue the anchor twice.
    assert_eq!(
        format!("{}{}", second.event.id, ledgers_audit::ANCHOR_ID_SUFFIX),
        format!("{}-anchor", second.event.id)
    );

    // A periodic job can anchor the current head on demand.
    let head = chain::read_head(db, company).await.expect("head");
    let statement = anchor_statement(company, &head, "run_01ANCHORJOB00000000001", AT)
        .expect("an anchor statement");
    db.execute(&statement).await.expect("anchor the head");
    assert_eq!(anchors(db, company).await, 2);
}

/// How many outbox rows mention `company` in their payload.
async fn anchors(db: &dyn Database, company: &str) -> usize {
    anchor_rows(db, company).await.len()
}

/// The outbox rows for `company`, as `(topic, payload)`, in id order.
async fn anchor_rows(db: &dyn Database, company: &str) -> Vec<(String, String)> {
    let rows = db
        .query(&Statement::new(format!(
            "SELECT topic, payload FROM {} WHERE payload LIKE '%\"company_id\":\"{company}\"%' \
             ORDER BY id",
            ledgers_db::outbox::OUTBOX_TABLE
        )))
        .await
        .expect("read the anchors");
    rows.rows
        .iter()
        .filter_map(|row| Some((row.get::<String>("topic")?, row.get::<String>("payload")?)))
        .collect()
}

/// Asserts that `sql` is refused, and that the reason is the
/// append-only rule rather than anything else (a syntax error would
/// "pass" this test too).
async fn assert_refused(db: &dyn Database, sql: &str) {
    let err = db
        .execute(&Statement::with_values(
            sql.to_owned(),
            vec![sea_query::Value::from(COMPANY_ID)],
        ))
        .await
        .expect_err("the audit chain must refuse this");
    let message = err.to_string().to_lowercase();
    assert!(
        message.contains("append-only") || message.contains("append only"),
        "the refusal must name the append-only rule, not fail for another reason: {err}"
    );
}

/// Detect if the database is Postgres by trying a Postgres-specific query.
async fn is_postgres(db: &dyn Database) -> bool {
    db.query(&Statement::new(
        "SELECT 1 FROM pg_catalog.pg_tables LIMIT 1".to_owned(),
    ))
    .await
    .is_ok()
}
