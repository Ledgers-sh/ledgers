//! The multi-backend conformance scenarios for the storage foundation:
//! plain `async` functions over `&dyn Database`, runnable against every
//! backing engine (D1, rusqlite, Postgres) with no runtime dependency —
//! whatever executor the caller has drives them. `tests/matrix.rs` runs
//! [`assert_storage_conformance`] over every dialect the environment
//! provides; a domain crate's storage tests layer their scenarios on the
//! same shape.

use cratefield_core::{Database, Statement};
use sea_query::{Alias, Expr, Query};

/// The single-company fixture, invented per AGENTS.md (never a real firm).
const COMPANY_NAME: &str = "Halden Works GmbH";
/// A valid ULID (the canonical example from the ULID spec), fixed so every
/// run writes and asserts the same row.
const COMPANY_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const BASE_CURRENCY: &str = "EUR";
const CREATED_AT: &str = "2026-01-15T08:30:00Z";
const UPDATED_AT: &str = "2026-01-15T08:30:00Z";
const SETTING_KEY: &str = "invoice_numbering";
const SETTING_VALUE: &str = "gapless";
const SETTING_UPDATED_AT: &str = "2026-01-16T09:00:00Z";

/// Probe DDL and fixtures for [`int64_round_trip`].
pub mod probe {
    /// The `BIGINT` probe table. `BIGINT` is deliberate — never `INTEGER`:
    /// Postgres `INTEGER` is 32-bit, while SQLite's type affinity maps
    /// `BIGINT` to a 64-bit integer, so one spelling stores `i64` on every
    /// backend. This is why every amount column in a domain migration
    /// spells `BIGINT`.
    pub const INT64_TABLE: &str = "CREATE TABLE IF NOT EXISTS int64_round_trip_probe \
         (id TEXT PRIMARY KEY, value BIGINT NOT NULL)";
    /// The row id carrying `i64::MAX`.
    pub const INT64_MAX_ID: &str = "i64-max";
    /// The row id carrying `i64::MIN`.
    pub const INT64_MIN_ID: &str = "i64-min";
}

/// Inserts the anchor `company` row and one `company_setting` row through
/// sea-query-rendered statements, then reads both back through the
/// `Database` port and asserts every field survives — proving the anchor
/// schema and the `company_id` scoping work identically on every backend.
///
/// Deletes its rows first and last, so it is rerunnable against a
/// persistent database.
///
/// # Panics
///
/// Panics when any statement fails or a read-back row differs from the
/// fixture — exactly what a conformance run should surface.
pub async fn company_round_trip(db: &dyn Database) {
    delete_fixtures(db).await;

    let mut insert = Query::insert();
    insert.into_table(Alias::new("company")).columns([
        Alias::new("id"),
        Alias::new("name"),
        Alias::new("base_currency"),
        Alias::new("created_at"),
        Alias::new("updated_at"),
    ]);
    insert
        .values([
            COMPANY_ID.into(),
            COMPANY_NAME.into(),
            BASE_CURRENCY.into(),
            CREATED_AT.into(),
            UPDATED_AT.into(),
        ])
        .expect("a value for each column, listed beside them");
    db.execute(&Statement::render(&insert))
        .await
        .expect("insert company row");

    let mut setting = Query::insert();
    setting.into_table(Alias::new("company_setting")).columns([
        Alias::new("company_id"),
        Alias::new("key"),
        Alias::new("value"),
        Alias::new("updated_at"),
    ]);
    setting
        .values([
            COMPANY_ID.into(),
            SETTING_KEY.into(),
            SETTING_VALUE.into(),
            SETTING_UPDATED_AT.into(),
        ])
        .expect("a value for each column, listed beside them");
    db.execute(&Statement::render(&setting))
        .await
        .expect("insert company_setting row");

    let mut select = Query::select();
    select
        .from(Alias::new("company"))
        .column(Alias::new("name"))
        .column(Alias::new("base_currency"))
        .column(Alias::new("created_at"))
        .column(Alias::new("updated_at"))
        .and_where(Expr::col(Alias::new("id")).eq(COMPANY_ID));
    let rows = db
        .query(&Statement::render(&select))
        .await
        .expect("select company by id");
    assert_eq!(rows.len(), 1, "one company row for the fixture id");
    let row = rows.first().expect("len checked above");
    assert_eq!(row.get::<String>("name"), Some(COMPANY_NAME.to_owned()));
    assert_eq!(
        row.get::<String>("base_currency"),
        Some(BASE_CURRENCY.to_owned())
    );
    assert_eq!(row.get::<String>("created_at"), Some(CREATED_AT.to_owned()));
    assert_eq!(row.get::<String>("updated_at"), Some(UPDATED_AT.to_owned()));

    // The setting reads back scoped to its company: the where clause names
    // both key parts of the composite primary key.
    let mut select_setting = Query::select();
    select_setting
        .from(Alias::new("company_setting"))
        .column(Alias::new("value"))
        .column(Alias::new("updated_at"))
        .and_where(Expr::col(Alias::new("company_id")).eq(COMPANY_ID))
        .and_where(Expr::col(Alias::new("key")).eq(SETTING_KEY));
    let rows = db
        .query(&Statement::render(&select_setting))
        .await
        .expect("select company_setting by company and key");
    assert_eq!(rows.len(), 1, "one setting row for the company and key");
    let row = rows.first().expect("len checked above");
    assert_eq!(row.get::<String>("value"), Some(SETTING_VALUE.to_owned()));
    assert_eq!(
        row.get::<String>("updated_at"),
        Some(SETTING_UPDATED_AT.to_owned())
    );

    delete_fixtures(db).await;
}

/// Proves the minor-units storage convention portably: `i64::MAX` and
/// `i64::MIN` bound as [`sea_query::Value::BigInt`], stored in a `BIGINT`
/// column and read back exactly, on every backend. A backend that narrows
/// the column (or the binder) to 32 bits fails here, not in a journal
/// entry.
///
/// # Panics
///
/// Panics when the probe table cannot be created, a statement fails, or a
/// value does not survive the round trip unchanged.
pub async fn int64_round_trip(db: &dyn Database) {
    db.execute(&Statement::new(probe::INT64_TABLE))
        .await
        .expect("create the int64 probe table");
    delete_probe(db).await;

    let mut insert = Query::insert();
    insert
        .into_table(Alias::new("int64_round_trip_probe"))
        .columns([Alias::new("id"), Alias::new("value")]);
    insert
        .values([
            probe::INT64_MAX_ID.into(),
            sea_query::Value::BigInt(Some(i64::MAX)).into(),
        ])
        .expect("a value for each column, listed beside them");
    db.execute(&Statement::render(&insert))
        .await
        .expect("insert the i64::MAX probe row");

    let mut insert = Query::insert();
    insert
        .into_table(Alias::new("int64_round_trip_probe"))
        .columns([Alias::new("id"), Alias::new("value")]);
    insert
        .values([
            probe::INT64_MIN_ID.into(),
            sea_query::Value::BigInt(Some(i64::MIN)).into(),
        ])
        .expect("a value for each column, listed beside them");
    db.execute(&Statement::render(&insert))
        .await
        .expect("insert the i64::MIN probe row");

    let mut select = Query::select();
    select
        .from(Alias::new("int64_round_trip_probe"))
        .column(Alias::new("id"))
        .column(Alias::new("value"))
        .order_by(Alias::new("id"), sea_query::Order::Asc);
    let rows = db
        .query(&Statement::render(&select))
        .await
        .expect("select the probe rows back");
    assert_eq!(rows.len(), 2, "both probe rows read back");
    let first = &rows.rows[0];
    assert_eq!(
        first.get::<String>("id"),
        Some(probe::INT64_MAX_ID.to_owned())
    );
    assert_eq!(first.get::<i64>("value"), Some(i64::MAX));
    let second = &rows.rows[1];
    assert_eq!(
        second.get::<String>("id"),
        Some(probe::INT64_MIN_ID.to_owned())
    );
    assert_eq!(second.get::<i64>("value"), Some(i64::MIN));

    delete_probe(db).await;
}

/// The composite every storage backend must pass: the harness port
/// contracts (document bytes round-trip, atomic batches) plus this
/// foundation's own scenarios. `tests/matrix.rs` runs it against every
/// dialect the environment provides.
///
/// # Panics
///
/// Panics when any constituent scenario panics.
pub async fn assert_storage_conformance(db: &dyn Database) {
    cratefield_testing::assert_blob_round_trips(db).await;
    cratefield_testing::assert_batch_is_atomic(db).await;
    company_round_trip(db).await;
    int64_round_trip(db).await;
}

/// Removes this run's fixture rows, setting first so the company row has
/// no referencing child when it goes.
async fn delete_fixtures(db: &dyn Database) {
    let mut settings = Query::delete();
    settings
        .from_table(Alias::new("company_setting"))
        .and_where(Expr::col(Alias::new("company_id")).eq(COMPANY_ID));
    db.execute(&Statement::render(&settings))
        .await
        .expect("delete fixture company_setting rows");

    let mut company = Query::delete();
    company
        .from_table(Alias::new("company"))
        .and_where(Expr::col(Alias::new("id")).eq(COMPANY_ID));
    db.execute(&Statement::render(&company))
        .await
        .expect("delete the fixture company row");
}

/// Removes the probe rows so the next run inserts into an empty table.
async fn delete_probe(db: &dyn Database) {
    db.execute(&Statement::new("DELETE FROM int64_round_trip_probe"))
        .await
        .expect("delete the int64 probe rows");
}
