//! The statements the list reads run on Cloudflare D1 as well as SQLite. The
//! Cloudflare port binds each parameter as a JavaScript value (`d1.rs` in
//! cratefield-runtime-cloudflare): an integer must be a JavaScript number, so
//! it has to sit inside the safe-integer range; a `u64` past that range, a
//! character, a JSON value or a timestamp type is not bound at all. SQLite
//! takes every one of them, so a query that binds `i64::MAX` passes the native
//! tests and fails only on Workers ("wasm-bindgen serialization error").
//!
//! This file runs the Settings and list commands on the native fixture through
//! a database wrapper that checks every bound parameter against that rule.

mod common;

use async_trait::async_trait;
use cratefield_core::{Database, DbError, Rows, Statement};
use ledgers_server::dispatch::{Deps, ServerDispatch};
use ledgers_server::routes::{Dispatch, Presented, SessionPresented};
use sea_query::Value as SeaValue;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use common::{Fixture, OWNER_SESSION, fixture_with_options, owner_session};

/// The app origin the owner session may call from.
const APP: &str = common::http::APP_ORIGIN;

/// The largest integer a D1 bind takes: 2^53 - 1.
const SAFE_MAX: i64 = (1 << 53) - 1;

/// Why one bound parameter would not reach D1 as the query expects, if it would not.
/// Every variant is listed, so a new one fails to compile until its D1 rule is written.
fn d1_problem(value: &SeaValue) -> Option<String> {
    let signed_ok = |v: i64| (-SAFE_MAX..=SAFE_MAX).contains(&v);
    match value {
        SeaValue::BigInt(Some(v)) if !signed_ok(*v) => {
            Some(format!("i64 {v} is outside the JavaScript safe range"))
        }
        SeaValue::BigUnsigned(Some(v)) if *v > SAFE_MAX as u64 => {
            Some(format!("u64 {v} is outside the JavaScript safe range"))
        }
        SeaValue::Double(Some(v)) if !v.is_finite() => {
            Some(format!("f64 {v} is not a finite number"))
        }
        SeaValue::Float(Some(v)) if !v.is_finite() => {
            Some(format!("f32 {v} is not a finite number"))
        }
        // The D1 port binds a character as NULL.
        SeaValue::Char(Some(c)) => Some(format!(
            "character {c:?} is not bound by the D1 port (it arrives as NULL)"
        )),
        SeaValue::Bool(_)
        | SeaValue::TinyInt(_)
        | SeaValue::SmallInt(_)
        | SeaValue::Int(_)
        | SeaValue::BigInt(_)
        | SeaValue::TinyUnsigned(_)
        | SeaValue::SmallUnsigned(_)
        | SeaValue::Unsigned(_)
        | SeaValue::BigUnsigned(_)
        | SeaValue::Float(_)
        | SeaValue::Double(_)
        | SeaValue::String(_)
        | SeaValue::Char(None)
        | SeaValue::Bytes(_) => None,
    }
}

/// The problems found on every statement the wrapped database ran.
#[derive(Default)]
struct Violations {
    statements: AtomicUsize,
    found: Mutex<Vec<String>>,
}

impl Violations {
    fn check(&self, stmt: &Statement) {
        self.statements.fetch_add(1, Ordering::SeqCst);
        let mut found = self.found.lock().expect("violations lock");
        for (index, value) in stmt.values.0.iter().enumerate() {
            if let Some(problem) = d1_problem(value) {
                found.push(format!("{} (parameter {index}): {problem}", stmt.sql));
            }
        }
    }
}

/// The database a command runs on, with every statement checked first.
struct BindCheck {
    inner: Arc<dyn Database>,
    violations: Arc<Violations>,
}

#[async_trait]
impl Database for BindCheck {
    async fn execute(&self, stmt: &Statement) -> Result<u64, DbError> {
        self.violations.check(stmt);
        self.inner.execute(stmt).await
    }

    async fn query(&self, stmt: &Statement) -> Result<Rows, DbError> {
        self.violations.check(stmt);
        self.inner.query(stmt).await
    }

    async fn batch_atomic(&self, stmts: &[Statement]) -> Result<(), DbError> {
        for stmt in stmts {
            self.violations.check(stmt);
        }
        self.inner.batch_atomic(stmts).await
    }
}

fn owned() -> Fixture {
    let fx = fixture_with_options(Vec::new(), None, vec![APP.to_owned()]);
    owner_session(&fx);
    fx
}

/// Runs one command as the owner session on a checked copy of the fixture's database.
fn run(
    fx: &Fixture,
    violations: &Arc<Violations>,
    command: &str,
    input: Value,
) -> Result<Value, String> {
    let mut deps: Deps = fx.deps.clone();
    deps.db = Arc::new(BindCheck {
        inner: deps.db.clone(),
        violations: violations.clone(),
    });
    let presented = Presented::Session(SessionPresented {
        id: OWNER_SESSION.to_owned(),
        origin: Some(APP.to_owned()),
        safe: true,
    });
    pollster::block_on(
        ServerDispatch::new(common::COMPANY)
            .with_app_origins(vec![APP.to_owned()])
            .dispatch(&deps, command, input, "req", Some(presented)),
    )
    .map_err(|problem| format!("{command} refused: {problem:?}"))
}

/// Every list and Settings read the app makes, with the filters it can send.
fn list_reads() -> Vec<(&'static str, Value)> {
    vec![
        ("audit.list", json!({})),
        ("audit.list", json!({"limit": 50})),
        ("audit.list", json!({"command": "entries.post"})),
        (
            "audit.list",
            json!({"outcome": "allowed", "actor_kind": "person"}),
        ),
        (
            "audit.list",
            json!({"from": "2026-01-01", "to": "2026-12-31"}),
        ),
        ("audit.list", json!({"before_seq": 2})),
        ("entries.list", json!({"limit": 50})),
        ("accounts.list", json!({})),
        ("approvals.list", json!({})),
        ("bills.list", json!({"limit": 100})),
        ("documents.list", json!({})),
        ("keys.list", json!({})),
        ("numbering.list", json!({})),
        (
            "reports.trends",
            json!({"from": "2026-01", "to": "2026-10"}),
        ),
        ("approval_settings.get", json!({})),
        ("tax_settings.get", json!({})),
        ("company.get", json!({})),
        ("fiscal_years.list", json!({})),
    ]
}

#[test]
fn list_reads_bind_only_values_d1_can_take() {
    let fx = owned();
    let violations = Arc::new(Violations::default());
    for (command, input) in list_reads() {
        run(&fx, &violations, command, input.clone()).unwrap_or_else(|err| panic!("{err}"));
    }
    let statements = violations.statements.load(Ordering::SeqCst);
    assert!(
        statements > 0,
        "the guard saw no statements; the wrapper is not in the path"
    );
    let found = violations.found.lock().expect("violations lock");
    assert!(
        found.is_empty(),
        "parameters D1 would refuse ({} statements checked):\n{}",
        statements,
        found.join("\n")
    );
}

#[test]
fn audit_first_page_binds_no_sequence_sentinel() {
    // Regression: the first page used to bind `i64::MAX` as its cursor.
    let fx = owned();
    let violations = Arc::new(Violations::default());
    run(
        &fx,
        &violations,
        "audit.list",
        json!({"command": "entries.post"}),
    )
    .expect("audit.list runs");
    let found = violations.found.lock().expect("violations lock");
    assert!(
        found.is_empty(),
        "the first audit page bound a value D1 refuses: {found:?}"
    );
}

#[test]
fn audit_refuses_a_cursor_past_the_safe_integer_range() {
    let fx = owned();
    let violations = Arc::new(Violations::default());
    let err = run(
        &fx,
        &violations,
        "audit.list",
        json!({"before_seq": SAFE_MAX + 2}),
    )
    .expect_err("a cursor D1 cannot bind is refused before any query");
    assert!(err.contains("before_seq"), "names the field: {err}");
    assert!(
        violations.found.lock().expect("violations lock").is_empty(),
        "nothing is bound for a refused cursor"
    );
}

#[test]
fn the_rule_accepts_the_safe_range_and_refuses_beyond_it() {
    assert!(d1_problem(&SeaValue::BigInt(Some(SAFE_MAX))).is_none());
    assert!(d1_problem(&SeaValue::BigInt(Some(-SAFE_MAX))).is_none());
    assert!(d1_problem(&SeaValue::BigInt(Some(i64::MAX))).is_some());
    assert!(d1_problem(&SeaValue::BigInt(Some(i64::MIN))).is_some());
    assert!(d1_problem(&SeaValue::BigUnsigned(Some(u64::MAX))).is_some());
    assert!(d1_problem(&SeaValue::BigInt(None)).is_none());
    assert!(d1_problem(&SeaValue::Char(Some('x'))).is_some());
    assert!(d1_problem(&SeaValue::Char(None)).is_none());
}
