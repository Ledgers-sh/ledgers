//! What one authenticated request costs in database work, and that nothing
//! carries over from one request to the next.
//!
//! Every D1 call is a network round trip on Workers, so the number of
//! statements a read runs is what its latency is made of. These tests pin
//! that number to a constant (it does not grow with the book), and check
//! that per-request counters and key checks never leak between requests.

mod common;

use common::{Fixture, entry_json, fixture};
use cratefield_core::{Config, Database, DbError, Rows, Statement};
use ledgers_server::Composed;
use ledgers_server::dispatch::{Deps, ServerDispatch};
use ledgers_server::rate_limiting::admit;
use ledgers_server::routes::{Dispatch, Presented};
use ledgers_server::timing::{self, CountingDb, DbTiming};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// The SQL of every statement a wrapped database runs, in order.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<String>>>);

impl Log {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().expect("log lock"))
    }
}

/// Records the SQL it forwards.
struct Recording {
    inner: Arc<dyn Database>,
    log: Log,
}

#[async_trait::async_trait]
impl Database for Recording {
    async fn execute(&self, stmt: &Statement) -> Result<u64, DbError> {
        self.log.0.lock().expect("log lock").push(stmt.sql.clone());
        self.inner.execute(stmt).await
    }
    async fn query(&self, stmt: &Statement) -> Result<Rows, DbError> {
        self.log.0.lock().expect("log lock").push(stmt.sql.clone());
        self.inner.query(stmt).await
    }
    async fn batch_atomic(&self, stmts: &[Statement]) -> Result<(), DbError> {
        for stmt in stmts {
            self.log.0.lock().expect("log lock").push(stmt.sql.clone());
        }
        self.inner.batch_atomic(stmts).await
    }
}

/// The fixture's ports with a counter and a statement log on the database,
/// as one request would see them.
struct Probe {
    deps: Deps,
    timing: Arc<DbTiming>,
    log: Log,
}

fn probe(fx: &Fixture) -> Probe {
    let timing = Arc::new(DbTiming::default());
    let log = Log::default();
    let mut deps = fx.deps.clone();
    let recording = Arc::new(Recording {
        inner: deps.db.clone(),
        log: log.clone(),
    });
    deps.db = Arc::new(CountingDb::new(recording, timing.clone()));
    Probe { deps, timing, log }
}

/// Runs one command as `key`, the way the HTTP adapters do.
fn run(probe: &Probe, key: &str, command: &str, input: Value) -> Value {
    pollster::block_on(ServerDispatch::new(common::COMPANY).dispatch(
        &probe.deps,
        command,
        input,
        "req",
        Some(Presented::Bearer(key.to_owned())),
    ))
    .unwrap_or_else(|problem| panic!("{command} refused: {problem:?}"))
}

/// The read commands the authenticated read path serves, with their inputs.
fn reads() -> Vec<(&'static str, Value)> {
    vec![
        ("accounts.get", json!({"number": "1030"})),
        ("entries.list", json!({"limit": 50})),
        ("entries.get", json!({"external_id": "ext-0"})),
        ("reports.trial_balance", json!({"period": "2026-10"})),
        (
            "reports.trends",
            json!({"from": "2026-01", "to": "2026-10", "dimension": "cost_centre"}),
        ),
    ]
}

/// One HTTP request's worth of work for `command`: admission (the rate
/// limit's charge), then dispatch. Same ports for both, as the router passes.
fn request(probe: &Probe, key: &str, command: &str, input: Value) -> Value {
    pollster::block_on(admit(
        &ServerDispatch::new(common::COMPANY),
        &probe.deps,
        Some(&Presented::Bearer(key.to_owned())),
    ))
    .expect("admitted");
    run(probe, key, command, input)
}

/// The statements one request for `command` ran, after one warm-up request
/// (the first verify of a key may rewrite `last_used_at`, once a minute).
fn statements_of(fx: &Fixture, command: &str, input: &Value) -> u64 {
    let warm = probe(fx);
    request(&warm, &fx.keys.read, command, input.clone());
    let measured = probe(fx);
    request(&measured, &fx.keys.read, command, input.clone());
    measured.timing.snapshot().statements
}

#[test]
fn admission_charges_the_key_without_reading_its_policy() {
    let fx = fixture();
    let p = probe(&fx);
    pollster::block_on(admit(
        &ServerDispatch::new(common::COMPANY),
        &p.deps,
        Some(&Presented::Bearer(fx.keys.read.clone())),
    ))
    .expect("admitted");
    let sql = p.log.take();
    assert!(
        sql.iter().all(|s| !s.contains("agent_key")),
        "admission read the policy row: {sql:#?}"
    );
    assert!(sql.len() <= 2, "admission ran {} statements", sql.len());
}

#[test]
fn authenticated_reads_run_the_same_statements_however_large_the_book() {
    let fx = fixture();
    let first = fx.post("ext-0", "5010", "4000", "100");
    assert_eq!(first.status.as_u16(), 200, "{}", first.json());
    let small: Vec<u64> = reads()
        .iter()
        .map(|(command, input)| statements_of(&fx, command, input))
        .collect();

    for n in 1..30 {
        let ext = format!("ext-{n}");
        let response = fx.post(&ext, "5010", "4000", "100");
        assert_eq!(response.status.as_u16(), 200, "{}", response.json());
    }
    let large: Vec<u64> = reads()
        .iter()
        .map(|(command, input)| statements_of(&fx, command, input))
        .collect();

    for (((command, _), before), after) in reads().iter().zip(&small).zip(&large) {
        assert_eq!(
            before, after,
            "{command} ran {before} statements on a small book and {after} on a larger one"
        );
    }
}

#[test]
fn authenticated_reads_stay_within_their_statement_budget() {
    // Per request: admission reads the key's secret row; dispatch reads it
    // again and the company's agent-key policy; the rest is the command. The
    // budgets are the measured counts, so a regression shows up as a number.
    let budgets: HashMap<&str, u64> = HashMap::from([
        ("accounts.get", 7),
        ("entries.list", 6),
        ("entries.get", 8),
        ("reports.trial_balance", 4),
        ("reports.trends", 9),
    ]);
    let fx = fixture();
    let posted = fx.post("ext-0", "5010", "4000", "100");
    assert_eq!(posted.status.as_u16(), 200, "{}", posted.json());
    for (command, input) in reads() {
        let count = statements_of(&fx, command, &input);
        assert!(
            count <= budgets[command],
            "{command} ran {count} statements, budget {}",
            budgets[command]
        );
    }
}

#[test]
fn a_read_loads_the_agent_key_once_and_not_again_to_authenticate() {
    let fx = fixture();
    let probe = probe(&fx);
    run(
        &probe,
        &fx.keys.read,
        "accounts.get",
        json!({"number": "1030"}),
    );
    let sql = probe.log.take();
    let policy_loads = sql
        .iter()
        .filter(|s| s.starts_with("SELECT id, policy FROM agent_key WHERE company_id"))
        .count();
    let key_row_loads = sql
        .iter()
        .filter(|s| s.contains("FROM agent_key WHERE company_id") && s.contains("AND id"))
        .count();
    assert_eq!(policy_loads, 1, "{sql:#?}");
    assert_eq!(key_row_loads, 0, "{sql:#?}");
}

#[test]
fn a_read_does_not_load_the_protected_account_set_but_a_write_does() {
    let fx = fixture();
    let read = probe(&fx);
    run(&read, &fx.keys.read, "entries.list", json!({"limit": 5}));
    let read_sql = read.log.take();
    assert!(
        read_sql.iter().all(|s| !s.contains("\"tags\"")),
        "a read loaded the chart: {read_sql:#?}"
    );

    // A write with the full key still reads the chart, because its tags
    // decide whether the posting needs approval.
    let write = probe(&fx);
    let body = entry_json("ext-write", "5010", "4000", "100");
    run(&write, &fx.keys.full, "entries.post", body);
    let write_sql = write.log.take();
    assert!(
        write_sql.iter().any(|s| s.contains("\"tags\"")),
        "a write skipped the protected-account check: {write_sql:#?}"
    );
}

#[test]
fn a_revoked_key_is_refused_on_the_next_request() {
    let fx = fixture();
    let first = probe(&fx);
    run(
        &first,
        &fx.keys.read,
        "accounts.get",
        json!({"number": "1030"}),
    );

    // Revoke every key row, then ask again through the same ports. Nothing
    // about the earlier, successful request may authorise this one.
    pollster::block_on(fx.deps.db.execute(&Statement::new(
        "UPDATE api_keys SET revoked_at = '2026-10-22T09:00:00Z'",
    )))
    .expect("revoke");
    let second = probe(&fx);
    let result = pollster::block_on(ServerDispatch::new(common::COMPANY).dispatch(
        &second.deps,
        "accounts.get",
        json!({"number": "1030"}),
        "req",
        Some(Presented::Bearer(fx.keys.read.clone())),
    ));
    assert!(result.is_err(), "a revoked key was accepted");
}

/// The statements one dispatch of `command` runs, after a warm-up dispatch.
fn dispatch_alone(fx: &Fixture, command: &str, input: &Value) -> u64 {
    let warm = probe(fx);
    run(&warm, &fx.keys.read, command, input.clone());
    let measured = probe(fx);
    run(&measured, &fx.keys.read, command, input.clone());
    measured.timing.snapshot().statements
}

#[test]
fn concurrent_requests_count_only_their_own_statements() {
    let fx = fixture();
    // The dispatch alone, as the interleaved calls below run it. The first
    // call warms the key's `last_used_at`, so the measured call is steady.
    let solo = dispatch_alone(&fx, "entries.list", &json!({"limit": 50}));

    let a = probe(&fx);
    let b = probe(&fx);
    let dispatch = ServerDispatch::new(common::COMPANY);
    let key = fx.keys.read.clone();
    // Interleave two different commands on one thread: the second request
    // runs while the first is between its statements.
    let (ra, rb) = pollster::block_on(futures_util::future::join(
        dispatch.dispatch(
            &a.deps,
            "entries.list",
            json!({"limit": 50}),
            "req",
            Some(Presented::Bearer(key.clone())),
        ),
        dispatch.dispatch(
            &b.deps,
            "accounts.get",
            json!({"number": "1030"}),
            "req",
            Some(Presented::Bearer(key)),
        ),
    ));
    assert!(ra.is_ok() && rb.is_ok());

    assert_eq!(a.timing.snapshot().statements, solo);
    let b_alone = dispatch_alone(&fx, "accounts.get", &json!({"number": "1030"}));
    assert_eq!(b.timing.snapshot().statements, b_alone);
    assert_ne!(
        solo, b_alone,
        "the two commands must differ for this to mean anything"
    );
}

#[test]
fn the_composed_app_is_built_once_for_any_number_of_requests() {
    static APP: Composed<Vec<u32>> = Composed::new();
    let builds = AtomicUsize::new(0);
    let mut seen = Vec::new();
    for _request in 0..25 {
        let app = APP.get_or_build(|| {
            builds.fetch_add(1, Ordering::SeqCst);
            (0..4).collect()
        });
        seen.push(std::ptr::from_ref(app));
    }
    assert_eq!(builds.load(Ordering::SeqCst), 1);
    assert!(seen.windows(2).all(|pair| pair[0] == pair[1]));
}

struct MapConfig(HashMap<String, String>);

impl Config for MapConfig {
    fn get(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
}

#[test]
fn timing_is_on_only_when_the_flag_is_one() {
    let on = |value: &str| {
        timing::enabled(&MapConfig(HashMap::from([(
            timing::TIMING_ENV.to_owned(),
            value.to_owned(),
        )])))
    };
    assert!(on("1"));
    assert!(!on("0"));
    assert!(!on("true"));
    assert!(!timing::enabled(&MapConfig(HashMap::new())));
}

#[test]
fn the_server_timing_value_names_the_three_database_metrics() {
    let fx = fixture();
    let p = probe(&fx);
    run(&p, &fx.keys.read, "accounts.get", json!({"number": "1030"}));
    let value = p.timing.server_timing();
    assert!(value.starts_with("db;dur="), "{value}");
    assert!(value.contains("db-calls;dur="), "{value}");
    assert!(
        value.contains(&format!(
            "db-statements;dur={}",
            p.timing.snapshot().statements
        )),
        "{value}"
    );
}

#[test]
fn a_trend_report_runs_the_same_statements_for_one_month_and_for_twelve() {
    let fx = fixture();
    let posted = fx.post("ext-0", "5010", "4000", "100");
    assert_eq!(posted.status.as_u16(), 200, "{}", posted.json());
    let one = json!({"from": "2026-10", "to": "2026-10", "dimension": "project"});
    let twelve = json!({"from": "2026-01", "to": "2026-12", "dimension": "project"});
    let short = statements_of(&fx, "reports.trends", &one);
    let long = statements_of(&fx, "reports.trends", &twelve);
    assert_eq!(short, long, "a month ran {short} statements, a year {long}");
}

#[test]
fn the_units_summary_costs_the_same_statements_however_many_units_it_compares() {
    // The side-by-side comparison is a fixed set of statements: one per
    // aggregate, whatever the number of business units.
    let fx = fixture();
    let person = ledgers_audit::AuditContext::new(
        ledgers_audit::Actor::Person {
            person_id: "person-owner".to_owned(),
        },
        "req-units",
        "01UNITSCOST000000000000000A".to_owned(),
    );
    let mut next = 0u32;
    let mut make = |count: u32| {
        for _ in 0..count {
            next += 1;
            let code = format!("U{next:03}");
            pollster::block_on(ledgers_ledger::units::create_unit(
                fx.deps.db.as_ref(),
                common::COMPANY,
                ledgers_ledger::units::UnitDraft {
                    code: code.clone(),
                    name: format!("Unit {code}"),
                    colour: None,
                    logo_document_id: None,
                },
                "2026-10-22T10:00:00Z".to_owned(),
                &person,
            ))
            .expect("the unit is created");
        }
    };
    make(2);
    let input = json!({"period": "2026-10"});
    let two = statements_of(&fx, "reports.units_summary", &input);
    make(6);
    let eight = statements_of(&fx, "reports.units_summary", &input);
    assert_eq!(
        two, eight,
        "the statement count grows with the units: {two} vs {eight}"
    );
}
