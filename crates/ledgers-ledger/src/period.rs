//! Fiscal years and accounting periods: the states, the human-only
//! transitions between them, and the year-end close — docs/PLAN.md §3
//! rule 4 ("closed periods reject postings; reopening is a human-only
//! action") and §5 (the policy that enforces it server-side).
//!
//! # States and where they live
//!
//! A period row lives in ledgers-db's `accounting_period` table (the
//! guard foundation owns the write-time check); its status domain is
//! the three constants this module mirrors as [`PeriodState`]:
//! `open` (anyone may post), `soft-closed` (people only — the
//! adjustment window state), `closed` (no one). A **missing row reads
//! as [`PeriodState::Closed`]**, mirroring the posting guard's
//! fail-closed scalar subquery: an unknown period is a closed period,
//! on both the write path and the read path.
//!
//! # Transitions are guarded writes, not read-then-write
//!
//! Every transition is one `batch_atomic` (ADR 0004): a guard probe
//! asserting the precondition **inside the batch**, the
//! [`ledgers_db::period::set_status_statement`] upsert, and the audit
//! append — so a stale pre-read can never land a wrong transition, and
//! a lost race fails the batch. The friendly pre-reads that exist are
//! for error messages, not for correctness.
//!
//! # People, agents, reasons
//!
//! Close and reopen are **human-only** ([`Actor`]; an agent key gets
//! [`AccountsError::HumanOnly`]) — enforced here, in the library, so no
//! adapter can route around it. A reopen needs a reason, audited; a
//! close blocked by registered checks can be forced by a person with an
//! override reason, audited with it (see [`crate::close`]).
//!
//! # The audit chain (interim)
//!
//! Each transition appends one event to ledgers-db's `audit_event`
//! spine: `seq = head.seq + 1`, `prev_hash` linked, and `hash` =
//! [`audit_hash`] — lowercase-hex SHA-256 over
//! `"ledgers-audit-v1|{seq}|{prev_hash}|{payload}|{created_at}"`.
//! Payloads are single-line `key=value` text ending with the reason
//! when one applies. **This scheme is interim**: `ledgers-audit` (#9)
//! owns hashing and verification, and may adopt this exact recipe or
//! migrate it — the statement of record for migrating is: hash the
//! pipe-joined five-field string with the `ledgers-audit-v1` tag,
//! seq-carrying, prev-hash-chaining, payload and created-at bound from
//! code; everything needed to re-derive or re-hash lives in the stored
//! row.
//!
//! # The year-end close
//!
//! [`close_fiscal_year`] closes the last period, clears every income
//! and expense account to zero through one deterministic closing entry
//! (posted person-side, so a soft-closed last period admits it), lands
//! the result in the tagged retained-earnings account, carries every
//! non-zero balance-sheet balance forward as the next year's opening
//! balances, and stamps the row — all in one final batch guarded on
//! `year_end_closed_at IS NULL`, which is what makes a rerun or a
//! concurrent second close idempotent instead of doubling the
//! carry-forward. The FX revaluation precondition of #42 is not
//! special-cased anywhere here: it will arrive as a registered
//! [`crate::close::CloseCheck`].

use crate::account::AccountType;
use crate::close::{Blocker, CloseCheckContext, CloseCheckRegistry, CloseReport, gather_blockers};
use crate::error::AccountsError;
use crate::fiscal::{self, FiscalYear, FiscalYearSpec, RETAINED_EARNINGS_TAG};
use crate::journal::{Journal, NewEntry, NewEntryLine, PostedEntry};
use crate::store;
use cratefield_core::{Database, DbError, Statement};
use ledgers_db::chain;
use ledgers_db::guard;
use ledgers_db::period as db_period;
use ledgers_db::retry::{self, RetryPolicy};
use ledgers_money::{Amount, Currency};
use sea_query::Value as SeaValue;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::Duration;

/// The daily post allowance the year-end closing entry posts under: a
/// person-driven system entry, given headroom so a close never trips a
/// limit meant for agents.
pub const YEAR_END_DAILY_LIMIT: u64 = 100_000;

/// The longest an override or reopen reason may be, in characters.
pub const MAX_REASON_LEN: usize = 512;

// ==== The state machine ====

/// One period's state, mirroring the `accounting_period` status domain
/// (`ledgers_db::period`'s `STATUS_*` constants). A period row that
/// does not exist reads as [`PeriodState::Closed`] — the fail-closed
/// mirror of the posting guard, which treats a missing row the same
/// way (its scalar subquery yields NULL and the guard fires).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum PeriodState {
    /// Anyone may post: people and agent keys.
    Open,
    /// Only a person may post: the soft close that stops agent keys
    /// while an adjustment window is still open for people.
    SoftClosed,
    /// No one may post: corrections are reversals in a later period.
    Closed,
}

impl PeriodState {
    /// The stored status string — exactly the `STATUS_*` constant of
    /// the same state, the single spelling both crates share.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => db_period::STATUS_OPEN,
            Self::SoftClosed => db_period::STATUS_SOFT_CLOSED,
            Self::Closed => db_period::STATUS_CLOSED,
        }
    }
}

impl std::fmt::Display for PeriodState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for PeriodState {
    type Err = AccountsError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            db_period::STATUS_OPEN => Ok(Self::Open),
            db_period::STATUS_SOFT_CLOSED => Ok(Self::SoftClosed),
            db_period::STATUS_CLOSED => Ok(Self::Closed),
            _ => Err(AccountsError::UnknownPeriodStatus {
                value: s.to_owned(),
            }),
        }
    }
}

/// Who is acting — the policy input every human-only transition checks.
/// Server-side, always: an agent key never becomes a person by asking
/// differently (docs/PLAN.md §5).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Actor {
    /// A person, identified by their user id.
    Person {
        /// The person's id.
        id: String,
    },
    /// An agent key.
    Agent {
        /// The key's id.
        key_id: String,
    },
}

impl Actor {
    /// Whether a person (not an agent key) is acting — the gate for
    /// close, reopen and year-end close.
    #[must_use]
    pub fn is_human(&self) -> bool {
        matches!(self, Self::Person { .. })
    }

    /// The audit subject label: `"person:{id}"` or `"agent:{key_id}"`.
    #[must_use]
    pub fn subject(&self) -> String {
        match self {
            Self::Person { id } => format!("person:{id}"),
            Self::Agent { key_id } => format!("agent:{key_id}"),
        }
    }
}

/// Validates a reason: trimmed, non-empty (else
/// [`AccountsError::ReasonRequired`]), at most [`MAX_REASON_LEN`]
/// characters and free of control characters (else
/// [`AccountsError::InvalidReason`]) so it can ride one audit line.
/// Returns the trimmed reason.
///
/// # Errors
///
/// [`AccountsError::ReasonRequired`] for empty/whitespace input;
/// [`AccountsError::InvalidReason`] when too long or control-laden.
#[must_use = "returns the trimmed reason or the refusal, never nothing"]
pub fn validate_reason(reason: &str) -> Result<&str, AccountsError> {
    let trimmed = reason.trim();
    if trimmed.is_empty() {
        return Err(AccountsError::ReasonRequired);
    }
    if trimmed.chars().count() > MAX_REASON_LEN {
        return Err(AccountsError::InvalidReason {
            reason: "longer than 512 characters",
        });
    }
    if trimmed.chars().any(char::is_control) {
        return Err(AccountsError::InvalidReason {
            reason: "contains control characters",
        });
    }
    Ok(trimmed)
}

// ==== The interim audit hash ====

/// The interim audit-event hash: lowercase hex SHA-256 over
/// `"ledgers-audit-v1|{seq}|{prev_hash}|{payload}|{created_at}"`. See
/// the module docs for the precise scheme `ledgers-audit` (#9) adopts
/// or migrates; the inputs are exactly the row's own columns.
fn audit_hash(seq: i64, prev_hash: &str, payload: &str, created_at: &str) -> String {
    let input = format!("ledgers-audit-v1|{seq}|{prev_hash}|{payload}|{created_at}");
    let digest = Sha256::digest(input.as_bytes());
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        // Writing to a String cannot fail; the result is discarded by
        // design, not by oversight.
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// The library's sleep: none. Retries re-plan without backing off —
/// the wasm-clean rule (ADR 0004) is that the library never sleeps or
/// blocks; callers that can block pass their own sleeper.
fn no_sleep(_: Duration) {}

// ==== Store helpers (raw statements, suite.rs style) ====

/// Every column of the `fiscal_year` table, for SELECTs.
const FISCAL_YEAR_COLUMNS: &str = "company_id, label, start_month, has_adjustment_period, \
                                   starts_on, ends_on, year_end_closed_at, closing_entry_id, \
                                   year_end_report, created_at, updated_at";

/// Reads one fiscal-year row, or `None` when the company has no such
/// label.
///
/// # Errors
///
/// [`AccountsError::Database`] when the query fails or a row cannot be
/// read back.
async fn read_fiscal_year_row(
    db: &dyn Database,
    company_id: &str,
    label: i64,
) -> Result<Option<FiscalYear>, AccountsError> {
    let rows = db
        .query(&Statement::with_values(
            format!(
                "SELECT {FISCAL_YEAR_COLUMNS} FROM fiscal_year WHERE company_id = ? AND label = ?"
            ),
            vec![SeaValue::from(company_id), SeaValue::BigInt(Some(label))],
        ))
        .await?;
    rows.first().map(FiscalYear::from_row).transpose()
}

/// Reads every fiscal-year row of the company, ascending by label.
///
/// # Errors
///
/// [`AccountsError::Database`] when the query fails or a row cannot be
/// read back.
async fn list_fiscal_year_rows(
    db: &dyn Database,
    company_id: &str,
) -> Result<Vec<FiscalYear>, AccountsError> {
    let rows = db
        .query(&Statement::with_values(
            format!(
                "SELECT {FISCAL_YEAR_COLUMNS} FROM fiscal_year WHERE company_id = ? ORDER BY label"
            ),
            vec![SeaValue::from(company_id)],
        ))
        .await?;
    rows.rows.iter().map(FiscalYear::from_row).collect()
}

/// The INSERT for one fiscal-year row (the close bookkeeping columns
/// stay NULL — they belong to the close).
fn insert_fiscal_year_statement(year: &FiscalYear) -> Statement {
    Statement::with_values(
        "INSERT INTO fiscal_year (company_id, label, start_month, has_adjustment_period, \
         starts_on, ends_on, created_at, updated_at)\n\
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
            .to_owned(),
        vec![
            SeaValue::from(year.company_id.as_str()),
            SeaValue::BigInt(Some(year.label)),
            SeaValue::BigInt(Some(i64::from(year.start_month))),
            SeaValue::BigInt(Some(i64::from(year.has_adjustment_period))),
            SeaValue::from(year.starts_on.as_str()),
            SeaValue::from(year.ends_on.as_str()),
            SeaValue::from(year.created_at.as_str()),
            SeaValue::from(year.updated_at.as_str()),
        ],
    )
}

/// The year-end UPDATE: stamps the close markers (the `IS NULL` guard
/// in the close batch is what keeps a second close out).
fn update_year_end_statement(
    company_id: &str,
    label: i64,
    closed_at: &str,
    closing_entry_id: &str,
    report: &str,
    updated_at: &str,
) -> Statement {
    Statement::with_values(
        "UPDATE fiscal_year SET year_end_closed_at = ?, closing_entry_id = ?, \
         year_end_report = ?, updated_at = ?\n\
         WHERE company_id = ? AND label = ?"
            .to_owned(),
        vec![
            SeaValue::from(closed_at),
            SeaValue::from(closing_entry_id),
            SeaValue::from(report),
            SeaValue::from(updated_at),
            SeaValue::from(company_id),
            SeaValue::BigInt(Some(label)),
        ],
    )
}

/// The INSERT of one carried-forward opening balance. Plain INSERT on
/// purpose: the primary key is the unique backstop a doubled close
/// trips, and the close's `IS NULL` guard should have refused it first.
fn opening_balance_statement(
    company_id: &str,
    fiscal_year: i64,
    account_number: &str,
    amount_minor: i64,
    created_at: &str,
) -> Statement {
    Statement::with_values(
        "INSERT INTO fiscal_year_opening_balance \
         (company_id, fiscal_year, account_number, amount_minor, created_at)\n\
         VALUES (?, ?, ?, ?, ?)"
            .to_owned(),
        vec![
            SeaValue::from(company_id),
            SeaValue::BigInt(Some(fiscal_year)),
            SeaValue::from(account_number),
            SeaValue::BigInt(Some(amount_minor)),
            SeaValue::from(created_at),
        ],
    )
}

/// How many of the company's fiscal years overlap the inclusive span
/// `starts_on..=ends_on` — the friendly pre-read; the in-batch guard in
/// [`open_fiscal_year`] is the enforcement.
///
/// The condition's two placeholders bind the NEW year's bounds in the
/// order the stored columns compare against: a stored row overlaps
/// when `stored.starts_on <= new.ends_on AND stored.ends_on >=
/// new.starts_on`.
///
/// # Errors
///
/// [`AccountsError::Database`] when the query fails.
async fn count_span_overlaps(
    db: &dyn Database,
    company_id: &str,
    starts_on: &str,
    ends_on: &str,
) -> Result<i64, AccountsError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT COUNT(*) AS n FROM fiscal_year\n\
             WHERE company_id = ? AND starts_on <= ? AND ends_on >= ?"
                .to_owned(),
            vec![
                SeaValue::from(company_id),
                // stored.starts_on <= ?
                SeaValue::from(ends_on),
                // stored.ends_on >= ?
                SeaValue::from(starts_on),
            ],
        ))
        .await?;
    Ok(rows
        .first()
        .and_then(|row| row.get::<i64>("n"))
        .unwrap_or_default())
}

/// Runs one guarded status transition with its audit append: guard
/// probe → upsert → probe release → chain append, re-reading the head
/// inside the build closure so a retry sequels onto a fresh chain.
async fn transition_with_audit(
    db: &dyn Database,
    company_id: &str,
    probe: Statement,
    probe_key: String,
    set_status: Statement,
    payload: &str,
    now: &str,
) -> Result<(), AccountsError> {
    retry::run_batch_with_retry(
        db,
        RetryPolicy::default(),
        async || {
            let head = chain::read_head(db, company_id).await?;
            let seq = head.seq + 1;
            let hash = audit_hash(seq, &head.hash, payload, now);
            Ok(vec![
                probe.clone(),
                set_status.clone(),
                guard::release_statement(&probe_key),
                chain::append_statement(company_id, &head, &hash, payload, now),
            ])
        },
        no_sleep,
    )
    .await?;
    Ok(())
}

// ==== The year-end report ====

/// What the year-end close did, in full: the report stored on the
/// `fiscal_year` row and returned verbatim by an idempotent rerun.
/// [`serialize`]/[`parse`] (single-line `key=value` text) are the
/// storage format.
///
/// [`serialize`]: Self::serialize
/// [`parse`]: Self::parse
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct YearEndReport {
    /// The company that closed.
    pub company_id: String,
    /// The fiscal year label that closed.
    pub label: i64,
    /// The closing entry as posted (deterministic id, gapless number).
    pub closing_entry: PostedEntry,
    /// The amount landed in retained earnings: the sum of the cleared
    /// income and expense balances, signed debit-positive (a profit is
    /// a credit, i.e. negative).
    pub retained_earnings_result: Amount,
    /// How many opening-balance rows were carried into the next year.
    pub opening_balance_count: usize,
    /// The blockers a person overrode to force this close (empty when
    /// the registry found nothing).
    pub overridden_blockers: Vec<Blocker>,
}

impl YearEndReport {
    /// Serializes the report as single-line `key=value` text — the
    /// `year_end_report` column format, and the audit-friendly shape
    /// the idempotent rerun parses back. `Amount` crosses as its
    /// canonical string.
    #[must_use]
    pub fn serialize(&self) -> String {
        let mut lines = vec![format!("company={}", self.company_id,)];
        lines.push(format!("label={}", self.label));
        lines.push(format!("closing-entry-id={}", self.closing_entry.entry_id));
        lines.push(format!(
            "closing-entry-number={}",
            self.closing_entry.number
        ));
        lines.push(format!(
            "closing-entry-date={}",
            self.closing_entry.entry_date
        ));
        lines.push(format!(
            "retained-earnings={}",
            self.retained_earnings_result.to_canonical_string()
        ));
        lines.push(format!("opening-balances={}", self.opening_balance_count));
        for blocker in &self.overridden_blockers {
            lines.push(format!(
                "blocker={}|{}|{}|{}",
                blocker.check_id, blocker.kind, blocker.count, blocker.link
            ));
        }
        lines.join("\n")
    }

    /// Parses a stored report back, `base` supplying the currency the
    /// canonical amount string re-hydrates in. The inverse of
    /// [`serialize`].
    ///
    /// # Errors
    ///
    /// [`AccountsError::Database`] (a `Query` error naming what was
    /// wrong) when a line is malformed or a required key is missing —
    /// a broken stored report, not a runtime condition.
    /// [`AccountsError::Money`] when the amount string no longer
    /// parses.
    pub fn parse(text: &str, base: Currency) -> Result<Self, AccountsError> {
        let corrupt = |what: &str| {
            AccountsError::Database(DbError::Query(format!(
                "stored year-end report does not parse: {what}"
            )))
        };
        let mut company_id = None;
        let mut label = None;
        let mut entry_id = None;
        let mut number = None;
        let mut entry_date = None;
        let mut retained = None;
        let mut count = None;
        let mut blockers = Vec::new();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                return Err(corrupt("a line without '='"));
            };
            match key {
                "company" => company_id = Some(value.to_owned()),
                "label" => {
                    label = Some(
                        value
                            .parse()
                            .map_err(|_| corrupt("label is not a number"))?,
                    );
                }
                "closing-entry-id" => entry_id = Some(value.to_owned()),
                "closing-entry-number" => {
                    number = Some(
                        value
                            .parse()
                            .map_err(|_| corrupt("number is not a number"))?,
                    );
                }
                "closing-entry-date" => entry_date = Some(value.to_owned()),
                "retained-earnings" => {
                    retained = Some(Amount::parse_canonical(value, base)?);
                }
                "opening-balances" => {
                    count = Some(
                        value
                            .parse()
                            .map_err(|_| corrupt("opening-balances is not a number"))?,
                    );
                }
                "blocker" => {
                    let parts: Vec<&str> = value.splitn(4, '|').collect();
                    if parts.len() != 4 {
                        return Err(corrupt("a blocker line without four fields"));
                    }
                    blockers.push(Blocker {
                        check_id: parts[0].to_owned(),
                        kind: parts[1].to_owned(),
                        count: parts[2]
                            .parse()
                            .map_err(|_| corrupt("a blocker count is not a number"))?,
                        link: parts[3].to_owned(),
                    });
                }
                _ => return Err(corrupt("unknown key")),
            }
        }
        Ok(Self {
            company_id: company_id.ok_or_else(|| corrupt("company missing"))?,
            label: label.ok_or_else(|| corrupt("label missing"))?,
            closing_entry: PostedEntry {
                entry_id: entry_id.ok_or_else(|| corrupt("closing-entry-id missing"))?,
                number: number.ok_or_else(|| corrupt("closing-entry-number missing"))?,
                entry_date: entry_date.ok_or_else(|| corrupt("closing-entry-date missing"))?,
            },
            retained_earnings_result: retained
                .ok_or_else(|| corrupt("retained-earnings missing"))?,
            opening_balance_count: count.ok_or_else(|| corrupt("opening-balances missing"))?,
            overridden_blockers: blockers,
        })
    }
}

// ==== Commands ====

/// Opens a fiscal year: validates the spec, refuses a duplicate label
/// or an overlapping span (pre-reads for the message; the batch's
/// overlap guard is the enforcement), then commits ONE batch — overlap
/// guard → `fiscal_year` insert → every period row materialized `open`.
/// Materializing is what makes posting possible at all: a missing
/// period row is closed by the posting guard's fail-closed design, so
/// [`open_fiscal_year`] is the only thing that ever opens a period.
///
/// # Errors
///
/// [`AccountsError::InvalidFiscalYear`] for a bad spec;
/// [`AccountsError::FiscalYearExists`] when the label is taken;
/// [`AccountsError::FiscalYearOverlap`] when the span touches an
/// existing year; [`AccountsError::Database`] when the batch fails.
pub async fn open_fiscal_year(
    db: &dyn Database,
    company_id: &str,
    spec: FiscalYearSpec,
    now: String,
) -> Result<FiscalYear, AccountsError> {
    spec.validate()?;
    if read_fiscal_year_row(db, company_id, spec.label)
        .await?
        .is_some()
    {
        return Err(AccountsError::FiscalYearExists { label: spec.label });
    }
    let starts_on = fiscal::starts_on(spec.label, spec.start_month);
    let ends_on = fiscal::ends_on(spec.label, spec.start_month);
    if count_span_overlaps(db, company_id, &starts_on, &ends_on).await? > 0 {
        return Err(AccountsError::FiscalYearOverlap {
            label: spec.label,
            starts_on,
            ends_on,
        });
    }

    let year = FiscalYear {
        company_id: company_id.to_owned(),
        label: spec.label,
        start_month: spec.start_month,
        has_adjustment_period: spec.has_adjustment_period,
        starts_on,
        ends_on,
        year_end_closed_at: None,
        closing_entry_id: None,
        year_end_report: None,
        created_at: now.clone(),
        updated_at: now,
    };
    let periods = fiscal::period_ids(&spec);
    let probe_key = guard::guard_key(&["fy-no-overlap", company_id, &spec.label.to_string()]);
    let probe = guard::assert_statement(
        &probe_key,
        "(SELECT COUNT(*) FROM fiscal_year WHERE company_id = ? AND starts_on <= ? AND ends_on >= ?) = 0",
        vec![
            SeaValue::from(company_id),
            // stored.starts_on <= new.ends_on, stored.ends_on >=
            // new.starts_on: the span-overlap predicate, bound in the
            // condition's placeholder order.
            SeaValue::from(year.ends_on.as_str()),
            SeaValue::from(year.starts_on.as_str()),
        ],
    );
    let insert = insert_fiscal_year_statement(&year);
    let open_statements: Vec<Statement> = periods
        .iter()
        .map(|period| db_period::set_status_statement(company_id, period, db_period::STATUS_OPEN))
        .collect();

    retry::run_batch_with_retry(
        db,
        RetryPolicy::default(),
        async || {
            let mut statements = Vec::with_capacity(3 + open_statements.len());
            statements.push(probe.clone());
            statements.push(insert.clone());
            statements.extend(open_statements.iter().cloned());
            statements.push(guard::release_statement(&probe_key));
            Ok(statements)
        },
        no_sleep,
    )
    .await?;
    Ok(year)
}

/// Soft-closes one period: people may still post (the adjustment
/// window), agent keys may not. Human-only; idempotent — soft-closing
/// a soft-closed period returns it unchanged. One guarded batch:
/// status must be `open` (the strict [`db_period::assert_open_statement`]
/// probe, in-batch) → set `soft-closed` → audit append.
///
/// # Errors
///
/// [`AccountsError::HumanOnly`] for an agent key;
/// [`AccountsError::PeriodStateInvalid`] when the period is closed
/// (including missing — fail-closed); [`AccountsError::Database`] when
/// the batch fails.
pub async fn soft_close_period(
    db: &dyn Database,
    company_id: &str,
    period: &str,
    actor: &Actor,
    now: String,
) -> Result<PeriodState, AccountsError> {
    if !actor.is_human() {
        return Err(AccountsError::HumanOnly {
            action: "soft_close_period".to_owned(),
        });
    }
    match period_state(db, company_id, period).await? {
        PeriodState::Open => (),
        already @ PeriodState::SoftClosed => return Ok(already),
        from @ PeriodState::Closed => {
            return Err(AccountsError::PeriodStateInvalid {
                period: period.to_owned(),
                from,
                to: PeriodState::SoftClosed,
            });
        }
    }
    let probe_key = guard::guard_key(&["period-soft-close", company_id, period]);
    let payload = format!(
        "action=soft-close-period company={company_id} period={period} actor={}",
        actor.subject()
    );
    transition_with_audit(
        db,
        company_id,
        db_period::assert_open_statement(&probe_key, company_id, period),
        probe_key,
        db_period::set_status_statement(company_id, period, db_period::STATUS_SOFT_CLOSED),
        &payload,
        &now,
    )
    .await?;
    Ok(PeriodState::SoftClosed)
}

/// Closes one period: nobody posts after this — corrections are
/// reversals in a later period. Human-only. Every registered close
/// check runs over the period's date range; any blocker refuses the
/// close with the complete report unless a person supplies an override
/// reason, which is then part of the audit payload. One guarded batch:
/// status must be `open` or `soft-closed` (the postable guard for a
/// human — the same disjunction, in-batch) → set `closed` → audit
/// append. Closing a closed period is a no-op returning `Closed`.
///
/// # Errors
///
/// [`AccountsError::HumanOnly`] for an agent key;
/// [`AccountsError::InvalidReason`] for a malformed override;
/// [`AccountsError::CloseBlocked`] when checks found blockers and no
/// override was given; [`AccountsError::Database`] when the batch
/// fails.
#[allow(clippy::too_many_arguments)]
pub async fn close_period(
    db: &dyn Database,
    company_id: &str,
    period: &str,
    journal: &dyn Journal,
    checks: &CloseCheckRegistry,
    override_reason: Option<&str>,
    actor: &Actor,
    now: String,
) -> Result<PeriodState, AccountsError> {
    if !actor.is_human() {
        return Err(AccountsError::HumanOnly {
            action: "close_period".to_owned(),
        });
    }
    let reason = match override_reason {
        Some(text) => Some(validate_reason(text)?),
        None => None,
    };
    if period_state(db, company_id, period).await? == PeriodState::Closed {
        return Ok(PeriodState::Closed);
    }

    let (starts_on, ends_on) = scan_range(db, company_id, period).await?;
    let blockers = gather_blockers(
        checks,
        CloseCheckContext {
            db,
            journal,
            company_id,
            period,
            starts_on: &starts_on,
            ends_on: &ends_on,
        },
    )
    .await?;
    let overridden = if blockers.is_empty() {
        Vec::new()
    } else if let Some(_reason) = reason {
        blockers
    } else {
        return Err(AccountsError::CloseBlocked {
            report: CloseReport {
                company_id: company_id.to_owned(),
                period: period.to_owned(),
                blockers,
            },
        });
    };
    // `overridden` is recorded in the audit payload below by count; the
    // full list lives in the year-end report when this is one.
    let blocker_note = if overridden.is_empty() {
        String::new()
    } else {
        format!(" overridden={}", overridden.len())
    };

    let probe_key = guard::guard_key(&["period-close", company_id, period]);
    let payload = match reason {
        Some(reason) => format!(
            "action=close-period company={company_id} period={period} actor={}\
             {blocker_note} override=1 reason={reason}",
            actor.subject()
        ),
        None => format!(
            "action=close-period company={company_id} period={period} actor={}{blocker_note}",
            actor.subject()
        ),
    };
    transition_with_audit(
        db,
        company_id,
        // The close precondition is exactly the posting guard's
        // person-side disjunction `open | soft-closed`: reusing the
        // shared spelling keeps one SQL source of truth for it.
        db_period::assert_postable_statement(&probe_key, company_id, period, true),
        probe_key,
        db_period::set_status_statement(company_id, period, db_period::STATUS_CLOSED),
        &payload,
        &now,
    )
    .await?;
    Ok(PeriodState::Closed)
}

/// Reopens one period: human-only, reason required and audited — the
/// rule 4 counterpart that makes closure reversible by people and by
/// nobody else. One guarded batch: status must be `soft-closed` or
/// `closed` (in-batch; a missing row fails the guard, so a period that
/// was never opened cannot be "reopened") → set `open` → audit append
/// carrying the reason.
///
/// # Errors
///
/// [`AccountsError::HumanOnly`] for an agent key;
/// [`AccountsError::ReasonRequired`]/[`AccountsError::InvalidReason`]
/// for a missing or malformed reason; [`AccountsError::PeriodStateInvalid`]
/// when the period is already open; [`AccountsError::Database`] when
/// the batch fails (including reopening a period with no row).
pub async fn reopen_period(
    db: &dyn Database,
    company_id: &str,
    period: &str,
    reason: &str,
    actor: &Actor,
    now: String,
) -> Result<PeriodState, AccountsError> {
    if !actor.is_human() {
        return Err(AccountsError::HumanOnly {
            action: "reopen_period".to_owned(),
        });
    }
    let reason = validate_reason(reason)?;
    if period_state(db, company_id, period).await? == PeriodState::Open {
        return Err(AccountsError::PeriodStateInvalid {
            period: period.to_owned(),
            from: PeriodState::Open,
            to: PeriodState::Open,
        });
    }
    let probe_key = guard::guard_key(&["period-reopen", company_id, period]);
    let probe = guard::assert_statement(
        &probe_key,
        &format!(
            "(SELECT status FROM accounting_period WHERE company_id = ? AND period = ?) \
             IN ('{}', '{}')",
            db_period::STATUS_SOFT_CLOSED,
            db_period::STATUS_CLOSED
        ),
        vec![SeaValue::from(company_id), SeaValue::from(period)],
    );
    let payload = format!(
        "action=reopen-period company={company_id} period={period} actor={} reason={reason}",
        actor.subject()
    );
    transition_with_audit(
        db,
        company_id,
        probe,
        probe_key,
        db_period::set_status_statement(company_id, period, db_period::STATUS_OPEN),
        &payload,
        &now,
    )
    .await?;
    Ok(PeriodState::Open)
}

/// Step 2 of the year-end close: every period except the last must be
/// `Closed`, and the last must be `Open` or `SoftClosed` — it receives
/// the closing entry, and the posting guard (not this read) is what
/// refuses a hard-closed one. Returns the last period's id.
///
/// # Errors
///
/// [`AccountsError::InvalidFiscalYear`] for a year with no periods;
/// [`AccountsError::PeriodStateInvalid`] for any wrong state;
/// [`AccountsError::Database`] when a read fails.
async fn last_closable_period(
    db: &dyn Database,
    company_id: &str,
    year: &FiscalYear,
) -> Result<String, AccountsError> {
    let periods = fiscal::period_ids(&year.spec());
    let last_period = periods
        .last()
        .ok_or_else(|| AccountsError::InvalidFiscalYear {
            reason: format!("fiscal year {} has no periods", year.label),
        })?
        .clone();
    for period in &periods[..periods.len() - 1] {
        let from = period_state(db, company_id, period).await?;
        if from != PeriodState::Closed {
            return Err(AccountsError::PeriodStateInvalid {
                period: period.clone(),
                from,
                to: PeriodState::Closed,
            });
        }
    }
    let from = period_state(db, company_id, &last_period).await?;
    if !matches!(from, PeriodState::Open | PeriodState::SoftClosed) {
        return Err(AccountsError::PeriodStateInvalid {
            period: last_period.clone(),
            from,
            to: PeriodState::Closed,
        });
    }
    Ok(last_period)
}

/// Steps 5 and 6 of the year-end close: the closing entry's lines — one
/// negating line per non-zero income/expense balance, plus the
/// balancing retained-earnings line — and the net they land (the year's
/// result, debit-positive: a profit is a credit, i.e. negative). All
/// checked `Amount` arithmetic; the posting batch's balanced guard is
/// the backstop.
///
/// # Errors
///
/// [`AccountsError::RetainedEarningsMissing`] when the chart has no
/// single usable retained-earnings account; [`AccountsError::Money`] /
/// [`AccountsError::Database`] otherwise.
async fn year_end_clearing(
    db: &dyn Database,
    journal: &dyn Journal,
    company_id: &str,
) -> Result<(Vec<NewEntryLine>, Amount), AccountsError> {
    // Step 5: exactly one leaf, unarchived retained-earnings account.
    let entries = store::list_account_entries(db, company_id).await?;
    let mut retained = entries.iter().filter(|entry| {
        entry.is_leaf() && !entry.account.archived && entry.account.has_tag(RETAINED_EARNINGS_TAG)
    });
    let Some(first) = retained.next() else {
        return Err(AccountsError::RetainedEarningsMissing {
            company_id: company_id.to_owned(),
        });
    };
    if retained.next().is_some() {
        return Err(AccountsError::RetainedEarningsMissing {
            company_id: company_id.to_owned(),
        });
    }
    let retained_number = first.account.number.clone();

    // Step 6: clear the P&L onto it.
    let base = journal.base_currency(db, company_id).await?;
    let by_number: HashMap<&str, &crate::account::Account> = entries
        .iter()
        .map(|entry| (entry.account.number.as_str(), &entry.account))
        .collect();
    let mut balances = journal.balances(db, company_id).await?;
    balances.sort_by(|a, b| a.account_number.cmp(&b.account_number));

    let mut net = Amount::new(0, base);
    let mut lines: Vec<NewEntryLine> = Vec::new();
    for balance in &balances {
        // Balances on accounts outside the chart (none in a consistent
        // ledger) have no class to clear by and no retained-earnings
        // counterpart defined; skipping keeps the close total.
        let Some(account) = by_number.get(balance.account_number.as_str()) else {
            continue;
        };
        let clears = matches!(
            account.account_type,
            AccountType::Income | AccountType::Expense
        );
        if clears && !balance.balance.is_zero() {
            net = net.checked_add(balance.balance)?;
            lines.push(NewEntryLine {
                account_number: balance.account_number.clone(),
                amount: balance.balance.checked_neg()?,
            });
        }
    }
    // The retained-earnings line is the balancing amount — the sum of
    // the cleared balances, landing as the year's result.
    lines.push(NewEntryLine {
        account_number: retained_number,
        amount: net,
    });
    Ok((lines, net))
}

/// Step 7: the opening balances the next year starts from — every
/// non-zero asset/liability/equity balance after the closing entry,
/// storage-boundary-ready (minor units as `i64`).
///
/// # Errors
///
/// [`AccountsError::Money`] / [`AccountsError::Database`].
async fn carried_forward_opening(
    db: &dyn Database,
    journal: &dyn Journal,
    company_id: &str,
) -> Result<Vec<(String, i64)>, AccountsError> {
    let entries = store::list_account_entries(db, company_id).await?;
    let by_number: HashMap<&str, &crate::account::Account> = entries
        .iter()
        .map(|entry| (entry.account.number.as_str(), &entry.account))
        .collect();
    let mut balances = journal.balances(db, company_id).await?;
    balances.sort_by(|a, b| a.account_number.cmp(&b.account_number));

    let mut opening = Vec::new();
    for balance in &balances {
        let Some(account) = by_number.get(balance.account_number.as_str()) else {
            continue;
        };
        let carries = matches!(
            account.account_type,
            AccountType::Asset | AccountType::Liability | AccountType::Equity
        );
        if carries && !balance.balance.is_zero() {
            opening.push((balance.account_number.clone(), balance.balance.to_i64()?));
        }
    }
    Ok(opening)
}

/// What step 8 commits: the year being closed, its closing entry, the
/// last period to close, and the stored report.
struct YearEndCommit<'a> {
    /// The fiscal-year label closing.
    label: i64,
    /// The posted closing entry's id.
    closing_entry_id: &'a str,
    /// The last period, which the batch closes.
    last_period: &'a str,
    /// The serialized report stamped onto the row.
    stored_report: &'a str,
}

/// Step 8: the one batch that commits the close — `year_end_closed_at
/// IS NULL` guard → opening-balance inserts → row stamp → last period
/// `closed` → audit append, with the head re-read inside the build
/// closure so a retry sequels onto a fresh chain. The guard is what
/// makes a rerun or a concurrent second close a no-op instead of a
/// doubled carry-forward.
///
/// # Errors
///
/// [`AccountsError::Conflict`] when the retry budget runs out;
/// [`AccountsError::Database`] when the batch fails (including a
/// second close winning the race).
async fn commit_year_end(
    db: &dyn Database,
    company_id: &str,
    commit: YearEndCommit<'_>,
    opening: &[(String, i64)],
    reason: Option<&str>,
    now: &str,
) -> Result<(), AccountsError> {
    let payload = match reason {
        Some(reason) => format!(
            "action=close-fiscal-year company={company_id} label={} \
             closing-entry={} opening-balances={} override=1 reason={reason}",
            commit.label,
            commit.closing_entry_id,
            opening.len()
        ),
        None => format!(
            "action=close-fiscal-year company={company_id} label={} \
             closing-entry={} opening-balances={}",
            commit.label,
            commit.closing_entry_id,
            opening.len()
        ),
    };
    let probe_key = guard::guard_key(&["yearend-open", company_id, &commit.label.to_string()]);
    let update = update_year_end_statement(
        company_id,
        commit.label,
        now,
        commit.closing_entry_id,
        commit.stored_report,
        now,
    );
    let opening_statements: Vec<Statement> = opening
        .iter()
        .map(|(number, minor)| {
            opening_balance_statement(company_id, commit.label + 1, number, *minor, now)
        })
        .collect();
    let close_last =
        db_period::set_status_statement(company_id, commit.last_period, db_period::STATUS_CLOSED);

    retry::run_batch_with_retry(
        db,
        RetryPolicy::default(),
        async || {
            let head = chain::read_head(db, company_id).await?;
            let seq = head.seq + 1;
            let hash = audit_hash(seq, &head.hash, &payload, now);
            let mut statements = Vec::with_capacity(5 + opening_statements.len());
            statements.push(guard::assert_statement(
                &probe_key,
                "(SELECT year_end_closed_at FROM fiscal_year WHERE company_id = ? AND label = ?) \
                 IS NULL",
                vec![
                    SeaValue::from(company_id),
                    SeaValue::BigInt(Some(commit.label)),
                ],
            ));
            statements.extend(opening_statements.iter().cloned());
            statements.push(update.clone());
            statements.push(close_last.clone());
            statements.push(guard::release_statement(&probe_key));
            statements.push(chain::append_statement(
                company_id, &head, &hash, &payload, now,
            ));
            Ok(statements)
        },
        no_sleep,
    )
    .await?;
    Ok(())
}

/// Closes a fiscal year end to end: clears the P&L, lands the result in
/// retained earnings, carries the balance sheet forward, closes the
/// last period, and stamps the row — idempotently.
///
/// The order of operations (each step documented where it happens):
///
/// 1. Human-only; an already-closed year returns its stored report
///    without writing (the idempotent rerun).
/// 2. Every period except the last must be `Closed`; the last must be
///    `Open` or `SoftClosed` (it receives the closing entry — the
///    posting guard refuses a hard-closed period, so the ordering is
///    enforced by the guard itself, not just by this read).
/// 3. The next fiscal year must exist: opening balances land there.
/// 4. The close checks run over the WHOLE year span; blockers refuse
///    unless a person overrides (the override is audited and reported).
/// 5. Exactly one leaf, unarchived account tagged `retained-earnings`.
/// 6. The closing entry: deterministic ids, dated `ends_on`, posted
///    person-side into the last period; one negating line per non-zero
///    income/expense balance, one balancing line on retained earnings;
///    skipped if it already exists (crash recovery).
/// 7. Post-closing balances carry forward: one opening-balance row per
///    non-zero asset/liability/equity account, for `label + 1`.
/// 8. ONE final batch: `year_end_closed_at IS NULL` guard → opening
///    balances → row stamp → last period `closed` → audit append (head
///    re-read inside the build closure).
///
/// # Errors
///
/// [`AccountsError::HumanOnly`] for an agent key;
/// [`AccountsError::FiscalYearMissing`] / [`AccountsError::NextFiscalYearMissing`]
/// for absent years; [`AccountsError::PeriodStateInvalid`] when the
/// period states are wrong; [`AccountsError::CloseBlocked`] for
/// unoverridden blockers; [`AccountsError::RetainedEarningsMissing`]
/// when the chart has no single usable retained-earnings account;
/// [`AccountsError::Money`] / [`AccountsError::Database`] otherwise.
#[allow(clippy::too_many_arguments)]
pub async fn close_fiscal_year(
    db: &dyn Database,
    company_id: &str,
    label: i64,
    journal: &dyn Journal,
    checks: &CloseCheckRegistry,
    override_reason: Option<&str>,
    actor: &Actor,
    now: String,
) -> Result<YearEndReport, AccountsError> {
    if !actor.is_human() {
        return Err(AccountsError::HumanOnly {
            action: "close_fiscal_year".to_owned(),
        });
    }
    let reason = match override_reason {
        Some(text) => Some(validate_reason(text)?),
        None => None,
    };
    let year = fiscal_year(db, company_id, label)
        .await?
        .ok_or(AccountsError::FiscalYearMissing { label })?;

    // 1. The idempotent rerun: a committed close returns its report and
    //    writes nothing at all.
    if year.year_end_closed_at.is_some() {
        let Some(stored) = year
            .year_end_report
            .as_deref()
            .filter(|text| !text.is_empty())
        else {
            return Err(AccountsError::Database(DbError::Query(format!(
                "fiscal year {label} is closed but stores no report"
            ))));
        };
        let base = journal.base_currency(db, company_id).await?;
        return YearEndReport::parse(stored, base);
    }

    // 2. The period states, and the last period to close.
    let last_period = last_closable_period(db, company_id, &year).await?;

    // 3. Opening balances need somewhere to land.
    if fiscal_year(db, company_id, label + 1).await?.is_none() {
        return Err(AccountsError::NextFiscalYearMissing { label: label + 1 });
    }

    // 4. The checks, over the whole year span (a draft dated in March
    //    blocks a December close just as much).
    let blockers = gather_blockers(
        checks,
        CloseCheckContext {
            db,
            journal,
            company_id,
            period: &last_period,
            starts_on: &year.starts_on,
            ends_on: &year.ends_on,
        },
    )
    .await?;
    let overridden_blockers = if blockers.is_empty() {
        Vec::new()
    } else if reason.is_some() {
        blockers
    } else {
        return Err(AccountsError::CloseBlocked {
            report: CloseReport {
                company_id: company_id.to_owned(),
                period: last_period.clone(),
                blockers,
            },
        });
    };

    // 5 + 6. The closing entry: clear the P&L onto retained earnings,
    //    posted person-side into the last period (deterministic ids,
    //    dated `ends_on`); skipped if it already exists (crash
    //    recovery).
    let (lines, net) = year_end_clearing(db, journal, company_id).await?;
    let entry = NewEntry {
        entry_id: format!("yearend-{company_id}-{label}"),
        journal_id: format!("jrn-yearend-{company_id}-{label}"),
        fiscal_year: label,
        entry_date: year.ends_on.clone(),
        period: Some(last_period.clone()),
        created_at: now.clone(),
        memo: format!("Year-end close of fiscal year {label}"),
        lines,
        actor_subject: actor.subject(),
        human: true,
        daily_limit: YEAR_END_DAILY_LIMIT,
    };
    let posted = match journal.find_entry(db, company_id, &entry.entry_id).await? {
        Some(existing) => existing,
        None => journal.post(db, &entry).await?,
    };

    // 7. Carry the balance sheet forward, from post-closing balances.
    let opening = carried_forward_opening(db, journal, company_id).await?;

    let report = YearEndReport {
        company_id: company_id.to_owned(),
        label,
        closing_entry: posted.clone(),
        retained_earnings_result: net,
        opening_balance_count: opening.len(),
        overridden_blockers,
    };

    // 8. The final batch: guarded on the close stamp, all or nothing.
    commit_year_end(
        db,
        company_id,
        YearEndCommit {
            label,
            closing_entry_id: &posted.entry_id,
            last_period: &last_period,
            stored_report: &report.serialize(),
        },
        &opening,
        reason,
        &now,
    )
    .await?;
    Ok(report)
}

// ==== Read helpers ====

/// Reads one fiscal year, or `None`.
///
/// # Errors
///
/// [`AccountsError::Database`] when the query fails or the row cannot
/// be read back.
pub async fn fiscal_year(
    db: &dyn Database,
    company_id: &str,
    label: i64,
) -> Result<Option<FiscalYear>, AccountsError> {
    read_fiscal_year_row(db, company_id, label).await
}

/// The fiscal year's periods with their current states, in fiscal
/// order — the close/reopen dashboard read.
///
/// # Errors
///
/// [`AccountsError::FiscalYearMissing`] when the year does not exist;
/// [`AccountsError::Database`] when a read fails.
pub async fn fiscal_year_periods(
    db: &dyn Database,
    company_id: &str,
    label: i64,
) -> Result<Vec<(String, PeriodState)>, AccountsError> {
    let Some(year) = read_fiscal_year_row(db, company_id, label).await? else {
        return Err(AccountsError::FiscalYearMissing { label });
    };
    let mut periods = Vec::new();
    for id in fiscal::period_ids(&year.spec()) {
        let state = period_state(db, company_id, &id).await?;
        periods.push((id, state));
    }
    Ok(periods)
}

/// One period's state; a missing row reads as [`PeriodState::Closed`]
/// (the fail-closed mirror — see the module docs).
///
/// # Errors
///
/// [`AccountsError::UnknownPeriodStatus`] when the stored status is
/// not one of the three; [`AccountsError::Database`] when the query
/// fails.
pub async fn period_state(
    db: &dyn Database,
    company_id: &str,
    period: &str,
) -> Result<PeriodState, AccountsError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT status FROM accounting_period WHERE company_id = ? AND period = ?".to_owned(),
            vec![SeaValue::from(company_id), SeaValue::from(period)],
        ))
        .await?;
    let Some(row) = rows.first() else {
        return Ok(PeriodState::Closed);
    };
    let status = row.get::<String>("status").ok_or_else(|| {
        AccountsError::Database(DbError::Query(
            "column status missing or not text".to_owned(),
        ))
    })?;
    status.parse()
}

/// The inclusive date range a close of `period` scans for blockers: the
/// period's own calendar month, or the single year-end date of the
/// owning fiscal year for an adjustment period (`"-13"`), which has no
/// calendar days of its own (see [`crate::fiscal`] for why).
///
/// # Errors
///
/// [`AccountsError::InvalidFiscalYear`] when `period` is not a period
/// id; [`AccountsError::FiscalYearMissing`] when an adjustment period
/// has no owning fiscal year.
async fn scan_range(
    db: &dyn Database,
    company_id: &str,
    period: &str,
) -> Result<(String, String), AccountsError> {
    let Some((year, month)) = fiscal::parse_period_id(period) else {
        return Err(AccountsError::InvalidFiscalYear {
            reason: format!("period {period:?} is not a period id"),
        });
    };
    if month <= 12 {
        return Ok(fiscal::calendar_month_range(year, month));
    }
    let owner = list_fiscal_year_rows(db, company_id)
        .await?
        .into_iter()
        .find(|candidate| fiscal::period_ids(&candidate.spec()).contains(&period.to_owned()));
    let Some(owner) = owner else {
        return Err(AccountsError::FiscalYearMissing { label: year });
    };
    Ok((owner.ends_on.clone(), owner.ends_on))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_states_round_trip_through_the_shared_constants() {
        for state in [
            PeriodState::Open,
            PeriodState::SoftClosed,
            PeriodState::Closed,
        ] {
            let text = state.to_string();
            assert_eq!(text, state.as_str());
            let parsed: PeriodState = text.parse().unwrap_or_else(|_| panic!("{text}"));
            assert_eq!(parsed, state);
        }
        assert_eq!(PeriodState::Open.as_str(), ledgers_db::period::STATUS_OPEN);
        assert_eq!(
            PeriodState::SoftClosed.as_str(),
            ledgers_db::period::STATUS_SOFT_CLOSED
        );
        assert_eq!(
            PeriodState::Closed.as_str(),
            ledgers_db::period::STATUS_CLOSED
        );
        assert!("sealed".parse::<PeriodState>().is_err());
    }

    #[test]
    fn the_actor_labels_subjects_and_gates_humans() {
        let person = Actor::Person {
            id: "owner".to_owned(),
        };
        let agent = Actor::Agent {
            key_id: "key-7".to_owned(),
        };
        assert!(person.is_human());
        assert!(!agent.is_human());
        assert_eq!(person.subject(), "person:owner");
        assert_eq!(agent.subject(), "agent:key-7");
    }

    #[test]
    fn reasons_must_be_present_trimmed_short_and_control_free() {
        assert_eq!(
            validate_reason("  operator reviewed  "),
            Ok("operator reviewed")
        );
        assert_eq!(validate_reason(""), Err(AccountsError::ReasonRequired));
        assert_eq!(validate_reason(" \t\n"), Err(AccountsError::ReasonRequired));
        let long = "x".repeat(MAX_REASON_LEN + 1);
        assert!(matches!(
            validate_reason(&long),
            Err(AccountsError::InvalidReason { .. })
        ));
        let exact = "x".repeat(MAX_REASON_LEN);
        assert_eq!(validate_reason(&exact), Ok(exact.as_str()));
        assert!(matches!(
            validate_reason("line\nbreak"),
            Err(AccountsError::InvalidReason { .. })
        ));
    }

    #[test]
    fn the_audit_hash_is_stable_and_chain_shaped() {
        let a = audit_hash(
            1,
            chain::GENESIS_HASH,
            "action=test",
            "2026-01-01T00:00:00Z",
        );
        let b = audit_hash(
            1,
            chain::GENESIS_HASH,
            "action=test",
            "2026-01-01T00:00:00Z",
        );
        assert_eq!(a, b, "deterministic");
        assert_eq!(a.len(), 64, "SHA-256 hex");
        assert!(
            a.bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
            "lowercase hex"
        );
        // Every field moves the hash: seq, prev_hash, payload, created_at.
        let moved = [
            audit_hash(
                2,
                chain::GENESIS_HASH,
                "action=test",
                "2026-01-01T00:00:00Z",
            ),
            audit_hash(1, "other", "action=test", "2026-01-01T00:00:00Z"),
            audit_hash(
                1,
                chain::GENESIS_HASH,
                "action=other",
                "2026-01-01T00:00:00Z",
            ),
            audit_hash(
                1,
                chain::GENESIS_HASH,
                "action=test",
                "2026-01-02T00:00:00Z",
            ),
        ];
        assert!(moved.iter().all(|hash| *hash != a));
    }

    #[test]
    fn the_year_end_report_round_trips_through_its_serialization() {
        let eur = Currency::from_code("EUR").expect("EUR is in the table");
        let report = YearEndReport {
            company_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_owned(),
            label: 2026,
            closing_entry: PostedEntry {
                entry_id: "yearend-01ARZ3NDEKTSV4RRFFQ69G5FAV-2026".to_owned(),
                number: 3,
                entry_date: "2026-12-31".to_owned(),
            },
            retained_earnings_result: Amount::from_major(-600, eur).expect("in range"),
            opening_balance_count: 2,
            overridden_blockers: vec![Blocker {
                check_id: "ledger.drafts".to_owned(),
                kind: "draft-entries".to_owned(),
                count: 2,
                link: "/companies/01ARZ3NDEKTSV4RRFFQ69G5FAV/entries/drafts?period=2026-12"
                    .to_owned(),
            }],
        };
        let text = report.serialize();
        assert!(text.starts_with("company=01ARZ3NDEKTSV4RRFFQ69G5FAV\nlabel=2026\n"));
        assert!(text.contains("retained-earnings=-600.00\n"));
        assert!(
            text.contains("?period=2026-12"),
            "a value may itself contain '='"
        );
        assert_eq!(
            YearEndReport::parse(&text, eur).expect("round trip"),
            report
        );
    }

    #[test]
    fn a_broken_stored_report_names_what_does_not_parse() {
        let eur = Currency::from_code("EUR").expect("EUR is in the table");
        let missing_key = "company=co\nlabel=2026\n";
        assert!(matches!(
            YearEndReport::parse(missing_key, eur),
            Err(AccountsError::Database(_))
        ));
        let no_equals = "company=co\nlabel=2026\ngarbage\n";
        assert!(matches!(
            YearEndReport::parse(no_equals, eur),
            Err(AccountsError::Database(_))
        ));
    }
}
