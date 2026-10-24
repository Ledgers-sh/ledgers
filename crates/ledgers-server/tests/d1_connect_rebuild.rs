//! The connections rebuild (migration 0002) as D1 applies it: the D1 mirror,
//! run as one batch inside a transaction, on a SQLite database that already
//! holds a connection with a credential and a sync run. The rows must survive,
//! the foreign-key check must be empty afterwards, and the widened connector
//! check must accept the cloud connectors.

use rusqlite::Connection;

/// The connections schema as migration 0001 creates it.
const CONNECT_0001: &str = include_str!("../../ledgers-connect/migrations/sqlite/0001_connect.sql");

/// The D1 mirror of migration 0002, exactly as committed.
const D1_CONNECT_PROVIDERS: &str =
    include_str!("../d1/migrations/0041_ledgers_connect_connect_providers.sql");

/// The D1 mirror of migration 0003 (the Google Cloud, Azure and Supabase ids).
const D1_CONNECT_CLOUD: &str =
    include_str!("../d1/migrations/0042_ledgers_connect_connect_cloud.sql");

fn seeded() -> Connection {
    let conn = Connection::open_in_memory().expect("in-memory sqlite");
    // D1 enforces foreign keys; so does this connection.
    conn.pragma_update(None, "foreign_keys", true)
        .expect("foreign keys on");
    conn.execute_batch(CONNECT_0001).expect("0001 applies");
    conn.execute_batch(
        "INSERT INTO connector_connection (id, company_id, connector, name, config_json, default_unit, is_demo, last_sync_at, deleted_at, created_by, created_at, updated_at)
         VALUES ('conn-1', 'co-1', 'cloudflare', 'Cloudflare (prod)', '{\"account_id\":\"abc\"}', 'FZ', 0, '2026-10-01T08:00:00Z', NULL, 'person-1', '2026-09-01T00:00:00Z', '2026-10-01T08:00:00Z');
         INSERT INTO connector_credential (connection_id, company_id, sealed, updated_at)
         VALUES ('conn-1', 'co-1', 'v1.sealed-value', '2026-09-01T00:00:00Z');
         INSERT INTO connector_run (id, company_id, connection_id, window_since, window_until, status, invoices_found, bills_created, usage_rows, error_message, notes_json, created_by, finished_at)
         VALUES ('run-1', 'co-1', 'conn-1', '2026-07-01', '2026-07-31', 'succeeded', 2, 2, 4, NULL, '[]', 'person-1', '2026-10-01T08:00:00Z');",
    )
    .expect("the existing rows are seeded");
    conn
}

#[test]
fn the_d1_rebuild_keeps_the_connection_its_credential_and_its_runs() {
    let conn = seeded();
    // D1 runs the file as one batch inside a transaction.
    conn.execute_batch(&format!("BEGIN;\n{D1_CONNECT_PROVIDERS}\nCOMMIT;"))
        .expect("the D1 mirror applies to a database with existing rows");

    let count = |sql: &str| -> i64 { conn.query_row(sql, [], |row| row.get(0)).expect("count") };
    assert_eq!(
        count(
            "SELECT COUNT(*) FROM connector_connection WHERE id = 'conn-1' AND name = 'Cloudflare (prod)'"
        ),
        1
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM connector_credential WHERE connection_id = 'conn-1'"),
        1
    );
    assert_eq!(
        count(
            "SELECT COUNT(*) FROM connector_run WHERE connection_id = 'conn-1' AND bills_created = 2"
        ),
        1
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM pragma_foreign_key_check"),
        0,
        "no child row refers to a missing parent"
    );

    // The widened check accepts the cloud connectors, and still refuses others.
    for connector in ["aws", "vercel"] {
        conn.execute(
            "INSERT INTO connector_connection (id, company_id, connector, name, config_json, default_unit, is_demo, created_by, created_at, updated_at)
             VALUES (?1, 'co-1', ?2, ?2, '{}', 'FZ', 0, 'person-1', 'now', 'now')",
            [format!("conn-{connector}"), connector.to_owned()],
        )
        .unwrap_or_else(|err| panic!("{connector} is accepted: {err}"));
    }
    let refused = conn.execute(
        "INSERT INTO connector_connection (id, company_id, connector, name, config_json, default_unit, is_demo, created_by, created_at, updated_at)
         VALUES ('conn-x', 'co-1', 'unknown', 'X', '{}', 'FZ', 0, 'person-1', 'now', 'now')",
        [],
    );
    assert!(refused.is_err(), "an unknown connector is still refused");
}

#[test]
fn the_cloud_mirror_applies_after_the_provider_mirror_to_a_database_with_rows() {
    let conn = seeded();
    conn.execute_batch(&format!("BEGIN;\n{D1_CONNECT_PROVIDERS}\nCOMMIT;"))
        .expect("0041 applies");
    conn.execute_batch(&format!("BEGIN;\n{D1_CONNECT_CLOUD}\nCOMMIT;"))
        .expect("0042 applies to the rebuilt table, with the run and credential rows in place");
    let count = |sql: &str| -> i64 { conn.query_row(sql, [], |row| row.get(0)).expect("count") };
    assert_eq!(
        count("SELECT COUNT(*) FROM connector_connection WHERE id = 'conn-1'"),
        1
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM connector_credential WHERE connection_id = 'conn-1'"),
        1
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM connector_run WHERE connection_id = 'conn-1'"),
        1
    );
    assert_eq!(count("SELECT COUNT(*) FROM pragma_foreign_key_check"), 0);
    for connector in ["gcp", "azure", "supabase"] {
        conn.execute(
            "INSERT INTO connector_connection (id, company_id, connector, name, config_json, default_unit, is_demo, created_by, created_at, updated_at)
             VALUES (?1, 'co-1', ?2, ?2, '{}', 'FZ', 0, 'person-1', 'now', 'now')",
            [format!("conn-{connector}"), connector.to_owned()],
        )
        .unwrap_or_else(|err| panic!("{connector} is accepted after 0042: {err}"));
    }
}

#[test]
fn a_failed_rebuild_rolls_back_and_leaves_the_books_as_they_were() {
    let conn = seeded();
    // A statement that fails after the rebuild started must undo all of it.
    let broken = format!("BEGIN;\n{D1_CONNECT_PROVIDERS}\nSELECT * FROM no_such_table;\nCOMMIT;");
    assert!(conn.execute_batch(&broken).is_err());
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM connector_connection", [], |row| {
            row.get(0)
        })
        .expect("count");
    assert_eq!(rows, 1, "the connection survives a failed rebuild");
    let runs: i64 = conn
        .query_row("SELECT COUNT(*) FROM connector_run", [], |row| row.get(0))
        .expect("count");
    assert_eq!(runs, 1);
}
