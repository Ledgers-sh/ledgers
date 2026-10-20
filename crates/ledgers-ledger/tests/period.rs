//! The period scenario matrix: fiscal years, the period states, the
//! human-only transitions and the year-end close, over every backing
//! engine the environment provides — in-memory SQLite always, a
//! throwaway Postgres 16 database when `FZ_TEST_POSTGRES_URL` names
//! one (CI's service container). One test definition, every dialect:
//! the same parity loop as `tests/matrix.rs` and
//! `ledgers-storage`'s conformance matrix, over the `ledgers-ledger`
//! AND `ledgers-db` schema together (the period rows live in the
//! guard foundation's tables).
//!
//! Posting goes through a **test-only** [`DemoJournal`]: the [`Journal`]
//! port driven by `ledgers-db`'s reference posting batch
//! (`suite::plan_post`, committed with one `batch_atomic`) over the
//! demo entry tables. That batch is the worked guard-statement example
//! — numbering, balanced, period-postable, daily-limit, audit chain —
//! so the scenarios exercise the real write-time refusals, not a
//! mock's opinions. The demo tables collapse the company scope onto
//! the journal id, so the [`DemoJournal`] binds `PostInput::journal_id`
//! to the scenario's company id (the `NewEntry::journal_id` field is
//! advisory here); the production journal crate (#6) scopes properly.
//! Draft state does not exist in the demo tables at all, so
//! `draft_count` reads an in-memory stand-in the scenarios feed.

use cratefield_core::{Database, Statement};
use cratefield_testing::TestHarness;
use ledgers_audit::{Actor, AuditContext, AuditEvent, Outcome};
use ledgers_db::DbModule;
use ledgers_db::chain;
use ledgers_db::suite::{DemoLine, PostInput, create_demo_tables, plan_post};
use ledgers_ledger::close::{
    Blocker, CloseCheck, CloseCheckContext, CloseCheckRegistry, DraftEntriesCloseCheck,
};
use ledgers_ledger::error::AccountsError;
use ledgers_ledger::fiscal::FiscalYearSpec;
use ledgers_ledger::journal_port::{
    AccountBalance, BoxFuture, Journal, NewEntry, NewEntryLine, PostedEntry,
};
use ledgers_ledger::period::{
    PeriodState, YearEndReport, close_fiscal_year, close_period, fiscal_year, fiscal_year_periods,
    open_fiscal_year, period_state, reopen_period, soft_close_period,
};
use ledgers_ledger::{LedgerModule, chart, commands};
use ledgers_money::{Amount, Currency};
use sea_query::Value as SeaValue;
use std::collections::HashMap;
use std::sync::Mutex;

/// The single-company fixture family, invented per AGENTS.md (never a
/// real firm): one clearly fake id per scenario, so every scenario owns
/// its own chart, fiscal years and period rows.
const LIFECYCLE_COMPANY: &str = "test-company-lifecycle";
const SOFT_CLOSE_COMPANY: &str = "test-company-soft-close";
const CHECKS_COMPANY: &str = "test-company-checks";
const POSTING_COMPANY: &str = "test-company-posting";
const REOPEN_COMPANY: &str = "test-company-reopen";
const YEAR_END_COMPANY: &str = "test-company-year-end";

const NOW: &str = "2026-02-01T09:00:00Z";

/// A small lint-clean chart declared inline (the same shape as
/// `tests/matrix.rs`): a bank under an assets heading, an income, an
/// expense, a liability and — load-bearing for the year-end close — an
/// equity account tagged `retained-earnings`.
const INLINE_CHART: &str = r#"
id = "inline"
name = "template.inline.name"
version = 1

[[accounts]]
number = "1010"
name = "account.inline.bank"
type = "asset"
parent = "1000"
tags = ["bank"]

[[accounts]]
number = "1000"
name = "account.inline.assets"
type = "asset"

[[accounts]]
number = "2000"
name = "account.inline.revenue"
type = "income"
tags = ["fx-gain"]

[[accounts]]
number = "3000"
name = "account.inline.costs"
type = "expense"
tags = ["fx-loss"]

[[accounts]]
number = "4000"
name = "account.inline.suspense"
type = "liability"
tags = ["suspense"]

[[accounts]]
number = "5000"
name = "account.inline.retained-earnings"
type = "equity"
tags = ["retained-earnings"]

[[accounts]]
number = "6000"
name = "account.inline.rounding"
type = "expense"
tags = ["rounding"]
"#;

fn eur() -> Currency {
    Currency::from_code("EUR").expect("EUR is in the currency table")
}

fn person() -> AuditContext {
    AuditContext::new(actor(true), request(), event_id())
}

fn agent() -> AuditContext {
    AuditContext::new(actor(false), request(), event_id())
}

/// The actor behind either context. The `AuditContext` carries it, so
/// a scenario picks one helper and the actor follows.
fn actor(human: bool) -> Actor {
    if human {
        Actor::Person {
            person_id: "owner-1".to_owned(),
        }
    } else {
        Actor::Agent {
            key_id: "key-1".to_owned(),
            tier: "post-within-limits".to_owned(),
        }
    }
}

fn request() -> String {
    "req_01PERIODSCENARIO0000000001".to_owned()
}

/// A fresh event id per call, so one company can be written to as
/// often as a scenario likes.
fn event_id() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "01PERIODSCENARIO{:012}",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

fn calendar_year(label: i64) -> FiscalYearSpec {
    FiscalYearSpec {
        label,
        start_month: 1,
        has_adjustment_period: false,
    }
}

/// The test-only journal: the [`Journal`] port over the demo entry
/// tables and the ledgers-db reference posting batch.
struct DemoJournal {
    /// The company every post is scoped to (the demo's journal id).
    company_id: String,
    /// Draft stand-ins: one inclusive `(from, to)` date range per
    /// drafted entry — the demo tables carry no draft state, so
    /// [`Journal::draft_count`] reads this instead.
    drafts: Mutex<Vec<(String, String)>>,
}

impl DemoJournal {
    /// A journal for `company_id` with no drafts.
    fn new(company_id: &str) -> Self {
        Self {
            company_id: company_id.to_owned(),
            drafts: Mutex::new(Vec::new()),
        }
    }

    /// Stands one drafted entry in `from..=to`.
    fn draft_entry_in(&self, from: &str, to: &str) {
        self.drafts
            .lock()
            .expect("drafts mutex poisoned")
            .push((from.to_owned(), to.to_owned()));
    }
}

impl Journal for DemoJournal {
    fn base_currency<'a>(
        &'a self,
        _db: &'a dyn Database,
        _company_id: &'a str,
    ) -> BoxFuture<'a, Result<Currency, AccountsError>> {
        Box::pin(async { Ok(eur()) })
    }

    fn balances<'a>(
        &'a self,
        db: &'a dyn Database,
        company_id: &'a str,
    ) -> BoxFuture<'a, Result<Vec<AccountBalance>, AccountsError>> {
        Box::pin(async move {
            let rows = db
                .query(&Statement::with_values(
                    "SELECT a.number AS account_number, CAST(COALESCE((
                        SELECT SUM(l.amount_minor) FROM demo_line l
                        JOIN demo_entry e ON e.entry_id = l.entry_id
                        WHERE e.journal_id = a.company_id AND l.account_number = a.number
                    ), 0) AS BIGINT) AS balance
                    FROM account a WHERE a.company_id = ? ORDER BY a.number"
                        .to_owned(),
                    vec![SeaValue::from(company_id)],
                ))
                .await?;
            let mut balances = Vec::new();
            for row in &rows.rows {
                balances.push(AccountBalance {
                    account_number: row
                        .get::<String>("account_number")
                        .expect("account_number is NOT NULL text"),
                    balance: Amount::from_i64(
                        row.get::<i64>("balance")
                            .expect("balance sums BIGINT lines"),
                        eur(),
                    ),
                });
            }
            Ok(balances)
        })
    }

    fn draft_count<'a>(
        &'a self,
        _db: &'a dyn Database,
        _company_id: &'a str,
        from: &'a str,
        to: &'a str,
    ) -> BoxFuture<'a, Result<i64, AccountsError>> {
        let drafts = self.drafts.lock().expect("drafts mutex poisoned").clone();
        Box::pin(async move {
            // ISO date strings compare chronologically; a draft counts
            // when its range intersects the requested one.
            Ok(i64::try_from(
                drafts
                    .iter()
                    .filter(|(draft_from, draft_to)| {
                        draft_from.as_str() <= to && draft_to.as_str() >= from
                    })
                    .count(),
            )
            .expect("a small draft count"))
        })
    }

    fn post<'a>(
        &'a self,
        db: &'a dyn Database,
        entry: &'a NewEntry,
        audit: &'a AuditContext,
    ) -> BoxFuture<'a, Result<PostedEntry, AccountsError>> {
        // The demo posts through ledgers-db's reference batch, which
        // carries no audit trail of its own; the event is appended here
        // so the period scenarios' chain reflects the port post.
        let _ = audit;
        Box::pin(async move {
            let mut lines = Vec::with_capacity(entry.lines.len());
            for (index, line) in entry.lines.iter().enumerate() {
                lines.push(DemoLine {
                    line_no: i64::try_from(index).expect("a small line index") + 1,
                    account_number: line.account_number.clone(),
                    currency: eur().code().to_owned(),
                    amount_minor: line.amount.to_i64()?,
                });
            }
            let input = PostInput {
                entry_id: entry.entry_id.clone(),
                // The demo collapses the company scope onto the journal
                // id, so the guard, numbering and chain all key on the
                // scenario's company.
                journal_id: self.company_id.clone(),
                fiscal_year: entry.fiscal_year,
                entry_date: entry.entry_date.clone(),
                created_at: entry.created_at.clone(),
                lines,
                human: entry.human,
                period_override: entry.period.clone(),
                agent_subject: entry.actor_subject.clone(),
                daily_limit: entry.daily_limit,
            };
            // The reference batch, planned and committed as-is. The
            // retry helper's `async ||` capture of `&PostInput` does not
            // prove `Send` once nested inside the port's boxed future,
            // and these sequential scenarios never race a chain head —
            // the first attempt is the only one, which is exactly
            // `batch_atomic` over `plan_post`.
            let statements = plan_post(db, &input).await?;
            db.batch_atomic(&statements).await?;
            let rows = db
                .query(&Statement::with_values(
                    "SELECT number, entry_date FROM demo_entry WHERE journal_id = ? AND entry_id = ?"
                        .to_owned(),
                    vec![
                        SeaValue::from(self.company_id.as_str()),
                        SeaValue::from(entry.entry_id.as_str()),
                    ],
                ))
                .await?;
            let row = rows
                .first()
                .expect("a committed post reads back by its own id");
            Ok(PostedEntry {
                entry_id: entry.entry_id.clone(),
                number: row.get::<i64>("number").expect("number is NOT NULL BIGINT"),
                entry_date: row
                    .get::<String>("entry_date")
                    .expect("entry_date is NOT NULL text"),
            })
        })
    }

    fn ensure_journal<'a>(
        &'a self,
        _db: &'a dyn Database,
        _company_id: &'a str,
        _journal_id: &'a str,
        _code: &'a str,
        _name: &'a str,
        _created_at: &'a str,
        _audit: &'a AuditContext,
    ) -> BoxFuture<'a, Result<(), AccountsError>> {
        // The demo collapses the journal onto the company: nothing to provision.
        Box::pin(async { Ok(()) })
    }

    fn find_entry<'a>(
        &'a self,
        db: &'a dyn Database,
        company_id: &'a str,
        entry_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<PostedEntry>, AccountsError>> {
        Box::pin(async move {
            let rows = db
                .query(&Statement::with_values(
                    "SELECT number, entry_date FROM demo_entry WHERE journal_id = ? AND entry_id = ?"
                        .to_owned(),
                    vec![
                        SeaValue::from(company_id),
                        SeaValue::from(entry_id),
                    ],
                ))
                .await?;
            Ok(rows.first().map(|row| PostedEntry {
                entry_id: entry_id.to_owned(),
                number: row.get::<i64>("number").expect("number is NOT NULL BIGINT"),
                entry_date: row
                    .get::<String>("entry_date")
                    .expect("entry_date is NOT NULL text"),
            }))
        })
    }
}

/// An FX-shaped close check for the scenarios: reports a configurable
/// count of unvalued positions, the way the real FX revaluation
/// precondition (#42) will register itself.
struct FxStubCheck {
    /// How many positions it reports as unvalued.
    pending: i64,
}

impl CloseCheck for FxStubCheck {
    fn id(&self) -> &'static str {
        "test.fx"
    }

    fn blockers<'a>(
        &'a self,
        ctx: CloseCheckContext<'a>,
    ) -> BoxFuture<'a, Result<Vec<Blocker>, AccountsError>> {
        let pending = self.pending;
        Box::pin(async move {
            if pending == 0 {
                return Ok(Vec::new());
            }
            Ok(vec![Blocker {
                check_id: "test.fx".to_owned(),
                kind: "fx-revaluation".to_owned(),
                count: pending,
                link: format!("/companies/{}/fx?period={}", ctx.company_id, ctx.period),
            }])
        })
    }
}

#[test]
fn period_scenarios_on_every_available_dialect() {
    for kit in TestHarness::all_dialects(|| vec![Box::new(LedgerModule), Box::new(DbModule)]) {
        // The scenarios panic with their own diagnosis on failure.
        pollster::block_on(scenarios(&*kit.db));
    }
}

/// The whole period machinery over one database: the lifecycle and
/// opening rules, the human-only transitions, the close checks, the
/// posting gates, the reopen contract and the year-end close.
///
/// # Panics
///
/// Panics when any step returns the wrong result — exactly what a
/// conformance run should surface.
pub async fn scenarios(db: &dyn Database) {
    lifecycle_and_opening_rules(db).await;
    soft_close_is_human_only(db).await;
    close_gathers_every_check_and_overrides_with_reason(db).await;
    posting_respects_period_states(db).await;
    reopen_is_human_reasoned_and_audited(db).await;
    year_end_close_end_to_end(db).await;
}

/// One scenario's fresh company: demo tables, the inline chart, and a
/// journal over both.
async fn setup(db: &dyn Database, company_id: &str) -> DemoJournal {
    create_demo_tables(db)
        .await
        .expect("create the demo tables");
    let template = chart::parse_chart(INLINE_CHART).expect("the inline chart parses");
    commands::import_template(db, company_id, &template, NOW.to_owned(), &person())
        .await
        .expect("import the inline chart");
    DemoJournal::new(company_id)
}

/// A balanced two-line entry for the scenarios: debit/credit pairs in
/// EUR minor units, dated and driven as given.
fn entry(
    company_id: &str,
    entry_id: &str,
    date: &str,
    period: Option<&str>,
    human: bool,
    actor: &AuditContext,
    lines: &[(&str, i128)],
) -> NewEntry {
    NewEntry {
        // The demo entry table keys its id globally (entry_id is the
        // bare PRIMARY KEY), so the scenario company scopes it here —
        // the real journal schema keys by (journal, id).
        entry_id: format!("{company_id}-{entry_id}"),
        journal_id: company_id.to_owned(),
        fiscal_year: 2026,
        entry_date: date.to_owned(),
        period: period.map(str::to_owned),
        created_at: NOW.to_owned(),
        memo: "scenario entry".to_owned(),
        lines: lines
            .iter()
            .map(|&(number, minor)| NewEntryLine {
                account_number: number.to_owned(),
                amount: Amount::new(minor, eur()),
            })
            .collect(),
        actor_subject: ledgers_ledger::subject(&actor.actor),
        human,
        daily_limit: 1000,
    }
}

/// The newest audit event on the company's chain, parsed out of its
/// JSON payload.
async fn last_audit_event(db: &dyn Database, company_id: &str) -> AuditEvent {
    // The demo journal's reference batch leaves its own placeholder
    // rows on the chain; the period commands append the real
    // `ledgers-audit` events after them. Walk back from the tail and
    // return the newest one that parses as an audit event.
    let rows = db
        .query(&Statement::with_values(
            "SELECT payload FROM audit_event WHERE company_id = ? ORDER BY seq DESC".to_owned(),
            vec![SeaValue::from(company_id)],
        ))
        .await
        .expect("read the audit tail");
    for row in &rows.rows {
        if let Some(payload) = row.get::<String>("payload")
            && let Ok(event) = serde_json::from_str::<AuditEvent>(&payload)
        {
            return event;
        }
    }
    panic!("no ledgers-audit event on {company_id}'s chain");
}

/// The audit events naming `command` on the company's chain.
async fn audit_events(db: &dyn Database, company_id: &str, command: &str) -> Vec<AuditEvent> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT payload FROM audit_event WHERE company_id = ? ORDER BY seq".to_owned(),
            vec![SeaValue::from(company_id)],
        ))
        .await
        .expect("read the audit chain");
    rows.rows
        .iter()
        .filter_map(|row| row.get::<String>("payload"))
        .filter_map(|payload| serde_json::from_str::<AuditEvent>(&payload).ok())
        .filter(|event| event.command == command)
        .collect()
}

/// The audit chain's head sequence for the company.
async fn head_seq(db: &dyn Database, company_id: &str) -> i64 {
    chain::read_head(db, company_id)
        .await
        .expect("read the audit head")
        .seq
}

/// How many posted entries the company holds.
async fn entry_count(db: &dyn Database, company_id: &str) -> i64 {
    let rows = db
        .query(&Statement::with_values(
            "SELECT COUNT(*) AS n FROM demo_entry WHERE journal_id = ?".to_owned(),
            vec![SeaValue::from(company_id)],
        ))
        .await
        .expect("count the posted entries");
    rows.first()
        .and_then(|row| row.get::<i64>("n"))
        .expect("COUNT always returns a row")
}

/// Opening rules: spans and labels compute, every period materializes
/// `open`, duplicate labels and overlapping spans refuse, an
/// adjustment period tiles its year plus one, and a period nobody
/// opened reads closed (the fail-closed mirror).
async fn lifecycle_and_opening_rules(db: &dyn Database) {
    let company = LIFECYCLE_COMPANY;
    setup(db, company).await;

    let year = open_fiscal_year(db, company, calendar_year(2026), NOW.to_owned(), &person())
        .await
        .expect("open fiscal 2026");
    assert_eq!(year.starts_on, "2026-01-01");
    assert_eq!(year.ends_on, "2026-12-31");
    assert_eq!(year.company_id, company);

    let periods = fiscal_year_periods(db, company, 2026)
        .await
        .expect("read the year's periods");
    assert_eq!(periods.len(), 12, "a calendar year has 12 periods");
    assert_eq!(periods.first().map(|(id, _)| id.as_str()), Some("2026-01"));
    assert_eq!(
        periods.last().map(|(id, _)| id.as_str()),
        Some("2026-12"),
        "{periods:?}"
    );
    assert!(
        periods.iter().all(|(_, state)| *state == PeriodState::Open),
        "a fresh year opens every period"
    );

    // A duplicate label refuses — the table is keyed by (company, label).
    assert_eq!(
        open_fiscal_year(db, company, calendar_year(2026), NOW.to_owned(), &person()).await,
        Err(AccountsError::FiscalYearExists { label: 2026 })
    );

    // A year starting December 2025 ends inside 2026: the spans share
    // days, so the overlap refuses.
    let overlapping = FiscalYearSpec {
        label: 2025,
        start_month: 12,
        has_adjustment_period: false,
    };
    let refused = open_fiscal_year(db, company, overlapping, NOW.to_owned(), &person())
        .await
        .expect_err("overlapping spans refuse");
    assert!(
        matches!(refused, AccountsError::FiscalYearOverlap { .. }),
        "{refused:?}"
    );

    // A July-start year with an adjustment period opens beside 2026:
    // 2027-07-01..2028-06-30 plus "2028-13", thirteen periods.
    let july = FiscalYearSpec {
        label: 2027,
        start_month: 7,
        has_adjustment_period: true,
    };
    open_fiscal_year(db, company, july, NOW.to_owned(), &person())
        .await
        .expect("open the July-start year");
    let july_periods = fiscal_year_periods(db, company, 2027)
        .await
        .expect("read the July year's periods");
    let ids: Vec<&str> = july_periods.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids.len(), 13);
    assert_eq!(ids[0], "2027-07");
    assert_eq!(ids[11], "2028-06");
    assert_eq!(ids[12], "2028-13", "the adjustment period is the 13th");

    // A period no fiscal year ever materialized reads closed.
    assert_eq!(
        period_state(db, company, "2030-01")
            .await
            .expect("read an unknown period"),
        PeriodState::Closed
    );
}

/// Soft close is a person's lever: an agent key is refused outright, a
/// person's soft close lands and audits, is idempotent, and cannot
/// reach into a hard-closed period.
async fn soft_close_is_human_only(db: &dyn Database) {
    let company = SOFT_CLOSE_COMPANY;
    let journal = setup(db, company).await;
    open_fiscal_year(db, company, calendar_year(2026), NOW.to_owned(), &person())
        .await
        .expect("open fiscal 2026");

    assert_eq!(
        soft_close_period(db, company, "2026-03", NOW.to_owned(), &agent()).await,
        Err(AccountsError::HumanOnly {
            action: "soft_close_period".to_owned()
        })
    );

    assert_eq!(
        soft_close_period(db, company, "2026-03", NOW.to_owned(), &person())
            .await
            .expect("a person soft-closes"),
        PeriodState::SoftClosed
    );
    assert_eq!(
        period_state(db, company, "2026-03")
            .await
            .expect("read the state back"),
        PeriodState::SoftClosed
    );

    let event = last_audit_event(db, company).await;
    assert_eq!(event.command, "period.soft_close", "{event:?}");
    assert_eq!(event.outcome, Outcome::Allowed, "{event:?}");
    assert_eq!(event.objects, ["2026-03"], "{event:?}");
    assert_eq!(event.actor, actor(true), "{event:?}");

    // Idempotent: soft-closing a soft-closed period changes nothing.
    assert_eq!(
        soft_close_period(db, company, "2026-03", NOW.to_owned(), &person())
            .await
            .expect("soft close is idempotent"),
        PeriodState::SoftClosed
    );

    // From hard-closed there is no soft close.
    close_period(
        db,
        company,
        "2026-03",
        &journal,
        &CloseCheckRegistry::new(),
        None,
        NOW.to_owned(),
        &person(),
    )
    .await
    .expect("close the soft-closed period");
    assert_eq!(
        soft_close_period(db, company, "2026-03", NOW.to_owned(), &person()).await,
        Err(AccountsError::PeriodStateInvalid {
            period: "2026-03".to_owned(),
            from: PeriodState::Closed,
            to: PeriodState::SoftClosed
        })
    );
}

/// Every registered check runs and every blocker is listed, in
/// registration order; the refusal writes nothing; a person's override
/// reason closes through and is audited with the payload; closing a
/// closed period is a no-op that writes nothing.
async fn close_gathers_every_check_and_overrides_with_reason(db: &dyn Database) {
    let company = CHECKS_COMPANY;
    let journal = setup(db, company).await;
    open_fiscal_year(db, company, calendar_year(2026), NOW.to_owned(), &person())
        .await
        .expect("open fiscal 2026");

    journal.draft_entry_in("2026-03-02", "2026-03-20");
    journal.draft_entry_in("2026-03-05", "2026-03-05");
    let mut checks = CloseCheckRegistry::new();
    checks.register(Box::new(DraftEntriesCloseCheck));
    checks.register(Box::new(FxStubCheck { pending: 1 }));

    let refused = close_period(
        db,
        company,
        "2026-03",
        &journal,
        &checks,
        None,
        NOW.to_owned(),
        &person(),
    )
    .await
    .expect_err("an unoverridden blocked close refuses");
    let AccountsError::CloseBlocked { report } = refused else {
        panic!("expected CloseBlocked, got {refused:?}")
    };
    assert_eq!(report.period, "2026-03");
    assert_eq!(report.company_id, company);
    assert_eq!(report.blockers.len(), 2, "every check's finding lists");
    assert_eq!(report.blockers[0].check_id, "ledger.drafts");
    assert_eq!(report.blockers[0].kind, "draft-entries");
    assert_eq!(report.blockers[0].count, 2, "both drafts count");
    assert!(
        report.blockers[0]
            .link
            .contains("/entries/drafts?period=2026-03"),
        "{}",
        report.blockers[0].link
    );
    assert_eq!(report.blockers[1].check_id, "test.fx");
    assert_eq!(report.blockers[1].kind, "fx-revaluation");
    assert_eq!(report.blockers[1].count, 1);

    // The refusal wrote nothing: the period is still open and the
    // chain has not moved.
    assert_eq!(
        period_state(db, company, "2026-03")
            .await
            .expect("state after the refusal"),
        PeriodState::Open
    );
    let seq_at_refusal = head_seq(db, company).await;

    // A person closes through with an override reason.
    close_period(
        db,
        company,
        "2026-03",
        &journal,
        &checks,
        Some("controller reviewed the open FX position"),
        NOW.to_owned(),
        &person(),
    )
    .await
    .expect("an overridden close lands");
    assert_eq!(
        period_state(db, company, "2026-03")
            .await
            .expect("state after the override"),
        PeriodState::Closed
    );
    let event = last_audit_event(db, company).await;
    assert_eq!(event.command, "period.close", "{event:?}");
    assert_eq!(event.outcome, Outcome::Allowed, "{event:?}");
    assert_eq!(event.objects, ["2026-03"], "{event:?}");
    assert_eq!(
        event.reason.as_deref(),
        Some("controller reviewed the open FX position"),
        "the override reason rides the event, not just its hash: {event:?}"
    );

    // Closing a closed period is a no-op: same answer, nothing written.
    let seq_after_close = head_seq(db, company).await;
    assert!(seq_after_close > seq_at_refusal);
    assert_eq!(
        close_period(
            db,
            company,
            "2026-03",
            &journal,
            &checks,
            None,
            NOW.to_owned(),
            &person(),
        )
        .await
        .expect("closing a closed period is a no-op"),
        PeriodState::Closed
    );
    assert_eq!(head_seq(db, company).await, seq_after_close);
}

/// Posting against the states: `open` admits everyone, `soft-closed`
/// admits a person and refuses an agent key, `closed` (and a period
/// nobody opened) refuses everyone — and every refusal rolls the whole
/// batch back, so the audit chain and the gapless numbering never move
/// for a refused post.
#[allow(clippy::too_many_lines)]
async fn posting_respects_period_states(db: &dyn Database) {
    let company = POSTING_COMPANY;
    let journal = setup(db, company).await;
    open_fiscal_year(db, company, calendar_year(2026), NOW.to_owned(), &person())
        .await
        .expect("open fiscal 2026");

    let sale = &[("1010", 50_000), ("2000", -50_000)][..];
    let first = journal
        .post(
            db,
            &entry(company, "e1", "2026-03-10", None, false, &agent(), sale),
            &person(),
        )
        .await
        .expect("an agent posts into an open period");
    assert_eq!(first.number, 1);
    let second = journal
        .post(
            db,
            &entry(company, "e2", "2026-03-11", None, true, &person(), sale),
            &person(),
        )
        .await
        .expect("a person posts into an open period");
    assert_eq!(second.number, 2);

    soft_close_period(db, company, "2026-03", NOW.to_owned(), &person())
        .await
        .expect("soft-close the period");

    let refused = journal
        .post(
            db,
            &entry(company, "e3", "2026-03-12", None, false, &agent(), sale),
            &person(),
        )
        .await;
    assert!(
        refused.is_err(),
        "a soft-closed period refuses an agent key"
    );

    let third = journal
        .post(
            db,
            &entry(company, "e3", "2026-03-12", None, true, &person(), sale),
            &person(),
        )
        .await
        .expect("a person still posts into a soft-closed period");
    assert_eq!(third.number, 3);

    close_period(
        db,
        company,
        "2026-03",
        &journal,
        &CloseCheckRegistry::new(),
        None,
        NOW.to_owned(),
        &person(),
    )
    .await
    .expect("hard-close the period");

    // Nobody posts into a hard-closed period, and the refusals leave
    // no trace: no chain event, no numbering hole.
    let seq_at_close = head_seq(db, company).await;
    for (label, human, actor) in [("agent", false, agent()), ("person", true, person())] {
        let refused = journal
            .post(
                db,
                &entry(company, "e4", "2026-03-20", None, human, &actor, sale),
                &person(),
            )
            .await;
        assert!(refused.is_err(), "a closed period refuses the {label}");
    }
    assert_eq!(head_seq(db, company).await, seq_at_close);

    let next = journal
        .post(
            db,
            &entry(company, "e5", "2026-04-02", None, true, &person(), sale),
            &person(),
        )
        .await
        .expect("the next open period still posts");
    assert_eq!(
        next.number, 4,
        "the refused attempts left no hole in the numbering"
    );

    // And an unknown period refuses everyone too (fail-closed).
    let adjustment = journal
        .post(
            db,
            &entry(
                company,
                "e6",
                "2026-04-03",
                Some("2026-13"),
                true,
                &person(),
                sale,
            ),
            &person(),
        )
        .await;
    assert!(adjustment.is_err(), "a period nobody opened refuses");

    posting_balances_read_back(&journal, db, company).await;
}

/// The scenario's posts read back from `balances` exactly: four sales
/// through the bank against the income account, nothing elsewhere.
async fn posting_balances_read_back(journal: &DemoJournal, db: &dyn Database, company: &str) {
    let balances: HashMap<String, Amount> = journal
        .balances(db, company)
        .await
        .expect("read the balances")
        .into_iter()
        .map(|balance| (balance.account_number, balance.balance))
        .collect();
    assert_eq!(balances["1010"], Amount::new(200_000, eur()));
    assert_eq!(balances["2000"], Amount::new(-200_000, eur()));
    assert_eq!(balances["5000"], Amount::new(0, eur()));
}

/// Reopen: an agent key cannot, a reason is mandatory (and trimmed),
/// an open period is not reopenable, a closed one reopens and audits
/// the reason, posting works again, and a period nobody opened fails
/// the in-batch guard rather than materializing.
#[allow(clippy::too_many_lines)]
async fn reopen_is_human_reasoned_and_audited(db: &dyn Database) {
    let company = REOPEN_COMPANY;
    let journal = setup(db, company).await;
    open_fiscal_year(db, company, calendar_year(2026), NOW.to_owned(), &person())
        .await
        .expect("open fiscal 2026");

    assert_eq!(
        reopen_period(
            db,
            company,
            "2026-02",
            "fix the invoice",
            NOW.to_owned(),
            &agent(),
        )
        .await,
        Err(AccountsError::HumanOnly {
            action: "reopen_period".to_owned()
        })
    );
    assert_eq!(
        reopen_period(db, company, "2026-02", "", NOW.to_owned(), &person()).await,
        Err(AccountsError::ReasonRequired)
    );
    assert_eq!(
        reopen_period(db, company, "2026-02", "   ", NOW.to_owned(), &person()).await,
        Err(AccountsError::ReasonRequired)
    );

    // An open period is not reopenable — nothing to undo.
    assert_eq!(
        reopen_period(
            db,
            company,
            "2026-02",
            "late invoice",
            NOW.to_owned(),
            &person(),
        )
        .await,
        Err(AccountsError::PeriodStateInvalid {
            period: "2026-02".to_owned(),
            from: PeriodState::Open,
            to: PeriodState::Open
        })
    );

    close_period(
        db,
        company,
        "2026-02",
        &journal,
        &CloseCheckRegistry::new(),
        None,
        NOW.to_owned(),
        &person(),
    )
    .await
    .expect("close February");

    assert_eq!(
        reopen_period(
            db,
            company,
            "2026-02",
            "  late supplier invoice arrived  ",
            NOW.to_owned(),
            &person(),
        )
        .await
        .expect("a person reopens with a reason"),
        PeriodState::Open
    );
    let event = last_audit_event(db, company).await;
    assert_eq!(event.command, "period.reopen", "{event:?}");
    assert_eq!(event.outcome, Outcome::Allowed, "{event:?}");
    assert_eq!(event.objects, ["2026-02"], "{event:?}");
    assert_eq!(
        event.reason.as_deref(),
        Some("late supplier invoice arrived"),
        "the audited reason is the trimmed one: {event:?}"
    );

    let posted = journal
        .post(
            db,
            &entry(
                company,
                "e1",
                "2026-02-20",
                None,
                true,
                &person(),
                &[("1010", 12_000), ("2000", -12_000)],
            ),
            &person(),
        )
        .await
        .expect("posting works again after the reopen");
    assert_eq!(posted.number, 1);

    // Reopening a period with no row fails the in-batch guard.
    let unknown = reopen_period(db, company, "2031-01", "probe", NOW.to_owned(), &person()).await;
    assert!(
        matches!(unknown, Err(AccountsError::Database(_))),
        "an unopened period fails the guard: {unknown:?}"
    );
}

/// The year-end close, end to end: prior periods must be closed, the
/// next year must exist, blockers refuse and a person overrides, the
/// closing entry clears the P&L into retained earnings, the balance
/// sheet carries into the next year's opening balances, the last
/// period closes, the row is stamped with its report — and the rerun
/// returns that report writing nothing at all.
async fn year_end_close_end_to_end(db: &dyn Database) {
    let company = YEAR_END_COMPANY;
    let journal = setup(db, company).await;
    open_fiscal_year(db, company, calendar_year(2026), NOW.to_owned(), &person())
        .await
        .expect("open fiscal 2026");

    // The year's activity: a 1 000.00 sale and 250.00 of costs, so the
    // year's result is a 750.00 profit (a credit on retained earnings).
    journal
        .post(
            db,
            &entry(
                company,
                "e1",
                "2026-03-10",
                None,
                true,
                &person(),
                &[("1010", 100_000), ("2000", -100_000)],
            ),
            &person(),
        )
        .await
        .expect("post the sale");
    journal
        .post(
            db,
            &entry(
                company,
                "e2",
                "2026-06-15",
                None,
                true,
                &person(),
                &[("3000", 25_000), ("1010", -25_000)],
            ),
            &person(),
        )
        .await
        .expect("post the costs");

    let checks = year_end_must_be_ready(db, company, &journal).await;
    let report = year_end_lands_and_carries(db, company, &journal, &checks).await;
    year_end_rerun_writes_nothing(db, company, &journal, &checks, &report).await;
}

/// The year-end close's preconditions, each refusing in turn: the
/// prior months must already be closed, the next fiscal year must
/// exist, and an unoverridden blocker refuses without writing.
/// Returns the registry for the close that follows.
async fn year_end_must_be_ready(
    db: &dyn Database,
    company: &str,
    journal: &DemoJournal,
) -> CloseCheckRegistry {
    // Close January through November: the year-end close insists.
    for month in 1..=11 {
        close_period(
            db,
            company,
            &format!("2026-{month:02}"),
            journal,
            &CloseCheckRegistry::new(),
            None,
            NOW.to_owned(),
            &person(),
        )
        .await
        .unwrap_or_else(|err| panic!("close 2026-{month:02}: {err}"));
    }

    // Without a next fiscal year there is nowhere to carry to.
    assert_eq!(
        close_fiscal_year(
            db,
            company,
            2026,
            journal,
            &CloseCheckRegistry::new(),
            None,
            NOW.to_owned(),
            &person(),
        )
        .await,
        Err(AccountsError::NextFiscalYearMissing { label: 2027 })
    );

    // The FX-shaped check blocks the year-end close too, and the
    // refusal is complete and writes nothing.
    let mut checks = CloseCheckRegistry::new();
    checks.register(Box::new(DraftEntriesCloseCheck));
    checks.register(Box::new(FxStubCheck { pending: 3 }));
    open_fiscal_year(db, company, calendar_year(2027), NOW.to_owned(), &person())
        .await
        .expect("open the next fiscal year");
    let refused = close_fiscal_year(
        db,
        company,
        2026,
        journal,
        &checks,
        None,
        NOW.to_owned(),
        &person(),
    )
    .await
    .expect_err("the FX check blocks the year-end close");
    let AccountsError::CloseBlocked { report } = refused else {
        panic!("expected CloseBlocked, got {refused:?}")
    };
    assert_eq!(
        report.period, "2026-12",
        "the year-end names its last period"
    );
    assert_eq!(report.blockers.len(), 1);
    assert_eq!(report.blockers[0].kind, "fx-revaluation");
    assert_eq!(report.blockers[0].count, 3);
    assert!(
        matches!(
            period_state(db, company, "2026-12").await,
            Ok(PeriodState::Open)
        ),
        "the refused close wrote nothing"
    );
    checks
}

/// The overridden close lands: the closing entry clears the P&L onto
/// retained earnings, the balance sheet carries into 2027's opening
/// balances, the last period closes, and the row is stamped with its
/// report. Returns the report for the rerun probe.
async fn year_end_lands_and_carries(
    db: &dyn Database,
    company: &str,
    journal: &DemoJournal,
    checks: &CloseCheckRegistry,
) -> YearEndReport {
    let report = close_fiscal_year(
        db,
        company,
        2026,
        journal,
        checks,
        Some("positions reviewed against year-end rates"),
        NOW.to_owned(),
        &person(),
    )
    .await
    .expect("the overridden year-end close lands");
    assert_eq!(report.company_id, company);
    assert_eq!(report.label, 2026);
    assert_eq!(
        report.closing_entry.entry_id,
        format!("yearend-{company}-2026")
    );
    assert_eq!(report.closing_entry.entry_date, "2026-12-31");
    assert_eq!(
        report.closing_entry.number, 3,
        "after the two scenario entries"
    );
    assert_eq!(report.retained_earnings_result, Amount::new(-75_000, eur()));
    assert_eq!(
        report.opening_balance_count, 2,
        "the bank and retained earnings"
    );
    assert_eq!(report.overridden_blockers.len(), 1);
    assert_eq!(report.overridden_blockers[0].kind, "fx-revaluation");

    // The P&L is cleared to zero and the result sits on retained
    // earnings; the balance sheet keeps its net.
    let balances: HashMap<String, Amount> = journal
        .balances(db, company)
        .await
        .expect("read the post-close balances")
        .into_iter()
        .map(|balance| (balance.account_number, balance.balance))
        .collect();
    assert!(balances["2000"].is_zero(), "income is cleared");
    assert!(balances["3000"].is_zero(), "expense is cleared");
    assert_eq!(balances["5000"], Amount::new(-75_000, eur()));
    assert_eq!(balances["1010"], Amount::new(75_000, eur()));

    // The last period closed; the row is stamped.
    assert_eq!(
        period_state(db, company, "2026-12")
            .await
            .expect("the last period's state"),
        PeriodState::Closed
    );
    let year = fiscal_year(db, company, 2026)
        .await
        .expect("read the year row")
        .expect("the year row exists");
    assert!(year.year_end_closed_at.is_some());
    assert_eq!(
        year.closing_entry_id.as_deref(),
        Some(report.closing_entry.entry_id.as_str())
    );

    // The carried-forward opening balances landed for 2027.
    assert_eq!(
        read_opening_balances(db, company, 2027).await,
        vec![("1010".to_owned(), 75_000), ("5000".to_owned(), -75_000),]
    );

    // The stored report parses back to exactly what was returned.
    let stored = year.year_end_report.expect("the row stores its report");
    assert_eq!(
        YearEndReport::parse(&stored, eur()).expect("parse it back"),
        report
    );
    report
}

/// The carried-forward opening-balance rows one fiscal year holds.
async fn read_opening_balances(
    db: &dyn Database,
    company: &str,
    fiscal_year: i64,
) -> Vec<(String, i64)> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT account_number, amount_minor FROM fiscal_year_opening_balance\n\
             WHERE company_id = ? AND fiscal_year = ? ORDER BY account_number"
                .to_owned(),
            vec![SeaValue::from(company), SeaValue::BigInt(Some(fiscal_year))],
        ))
        .await
        .expect("read the opening balances");
    rows.rows
        .iter()
        .map(|row| {
            (
                row.get::<String>("account_number")
                    .expect("account_number is NOT NULL text"),
                row.get::<i64>("amount_minor")
                    .expect("amount_minor is BIGINT"),
            )
        })
        .collect()
}

/// The rerun is idempotent — the stored report returns, nothing is
/// written — and the year-end audit payload carries the override.
async fn year_end_rerun_writes_nothing(
    db: &dyn Database,
    company: &str,
    journal: &DemoJournal,
    checks: &CloseCheckRegistry,
    report: &YearEndReport,
) {
    // The rerun is idempotent: the same report, nothing written.
    let entries_before = entry_count(db, company).await;
    let seq_before = head_seq(db, company).await;
    let again = close_fiscal_year(
        db,
        company,
        2026,
        journal,
        checks,
        None,
        NOW.to_owned(),
        &person(),
    )
    .await
    .expect("the rerun returns the stored report");
    assert_eq!(&again, report);
    assert_eq!(entry_count(db, company).await, entries_before);
    assert_eq!(head_seq(db, company).await, seq_before);

    // The year-end event audited the override too.
    let closes = audit_events(db, company, "fiscal_year.close").await;
    let event = closes.first().expect("the year-end close audited");
    assert_eq!(event.outcome, Outcome::Allowed, "{event:?}");
    assert_eq!(
        event.objects,
        vec!["2026".to_owned(), report.closing_entry.entry_id.clone()],
        "the year and the closing entry it posted: {event:?}"
    );
    assert_eq!(
        event.reason.as_deref(),
        Some("positions reviewed against year-end rates"),
        "{event:?}"
    );
}
