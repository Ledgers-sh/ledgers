//! Posting: the one `batch_atomic` that turns a validated [`Entry`]
//! into a posted, numbered, audit-chained, event-carrying row — and the
//! proof that nothing else can (docs/PLAN.md §3 rules 2, 3, 5).
//!
//! The batch's statement order (the mechanics and the why are
//! docs/adr/0004-guard-statements.md, which this module implements and
//! does not re-narrate):
//!
//! ```sql
//! -- 1. numbering: provision (idempotent), bump, insert reading the counter
//! -- 2. the entry's lines
//! -- 3. journal-ownership guard (the journal belongs to the posting company)
//! -- 4. reversal-target guard, when the entry reverses one (same company)
//! -- 5. period-open guard (fails closed: unknown period = closed)
//! -- 6. accounts guard (every line's account exists, none archived)
//! -- 7. balanced guard (>= 2 lines; base sums to 0; a single tx currency sums to 0)
//! -- 8. releases for every probe
//! -- 9. the audit append (claims head.seq + 1; the PK makes forks impossible)
//! -- 10. the outbox enqueue (journal.entry.posted)
//! ```
//!
//! Everything commits together or not at all: a refused entry consumes
//! no number (the bump rolls back with the refusal), writes no lines,
//! appends no audit event, enqueues nothing. Retry re-plans the batch
//! from fresh reads ([`ledgers_db::retry::run_batch_with_retry`]); a
//! guard refusal is final, and the posting entry points map it back to
//! a domain error with diagnostic reads (allowed by the ADR: reads
//! after a failed write choose the error, they never gate one).
//!
//! There is no update and no delete for `journal_entry` or
//! `journal_line` anywhere in this crate: corrections are reversals —
//! new entries whose `reverses` names the original, read back through
//! [`crate::entry::reversed_by`].

use crate::entry::{Entry, JournalError, Line, iso_date_string};
use cratefield_core::{Database, DbError, Statement};
use ledgers_audit::{AuditConfig, AuditContext, AuditRecord, Outcome, append_statements};
use ledgers_db::chain;
use ledgers_db::guard;
use ledgers_db::limit;
use ledgers_db::numbering;
use ledgers_db::outbox::OUTBOX_TABLE;
use ledgers_db::period::{self, assert_postable_statement};
use ledgers_db::retry::{self, RetryPolicy};
use ledgers_money::Currency;
use sea_query::Value as SeaValue;
use std::collections::BTreeMap;
use std::time::Duration;
use time::{Date, Month, OffsetDateTime};

/// The outbox topic a posted entry announces itself on.
pub const TOPIC_ENTRY_POSTED: &str = "journal.entry.posted";

/// The audited command name a post records under.
const COMMAND_POST: &str = "journal.post_entry";

/// The usage meter the port-level daily allowance is charged under. The
/// same name `ledgers_db::suite` posts under.
const METER_POST: &str = "post";

/// What a port-level post adds to the entries batch. The default is the
/// entries API's own posting: the period the posting date names, admitted
/// only while `open`, with no daily allowance.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PostOptions {
    /// The period to post into instead of the one the posting date
    /// names: the one way an entry names an adjustment period.
    pub period_override: Option<String>,
    /// Whether a person drives the post: a `soft-closed` period admits
    /// person posts only (`ledgers_db::period::assert_postable_statement`).
    pub human: bool,
    /// `(subject, limit)`: one post is charged to `subject` under the
    /// `post` meter, which refuses past `limit` a UTC day.
    pub daily_limit: Option<(String, u64)>,
}

/// The entry table, as declared by [`crate::module::LedgerModule`].
const ENTRY_TABLE: &str = "journal_entry";
/// The line table, as declared by [`crate::module::LedgerModule`].
const LINE_TABLE: &str = "journal_line";

/// One line as storage sees it: signed minor units, both legs, no
/// domain guarantees. Test scaffolding for [`RawEntry`].
#[doc(hidden)]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RawLine {
    /// The account's number in the chart.
    pub account_number: String,
    /// The base-currency code (`"EUR"`).
    pub base_currency: String,
    /// Signed base minor units: debit `+`, credit `−`.
    pub base_minor: i64,
    /// The transaction-currency code; the base code without an FX leg.
    pub tx_currency: String,
    /// Signed transaction minor units.
    pub tx_minor: i64,
    /// The FX rate's scaled value, when the line has one.
    pub rate_scaled: Option<String>,
    /// The tax code, when the line carries one.
    pub tax_code: Option<String>,
    /// Reporting dimensions, JSON on the wire.
    pub dimensions: BTreeMap<String, String>,
    /// The human note.
    pub memo: Option<String>,
}

/// An entry as storage sees it: ISO dates, JSON documents, no
/// validation. Test scaffolding for [`post_raw_entry`].
#[doc(hidden)]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RawEntry {
    /// The company whose books the entry enters.
    pub company_id: String,
    /// The journal the entry posts into.
    pub journal_id: String,
    /// ISO document date.
    pub document_date: String,
    /// ISO posting date.
    pub posting_date: String,
    /// ISO tax point date.
    pub tax_point_date: String,
    /// The base-currency code.
    pub base_currency: String,
    /// The human description.
    pub description: String,
    /// Source-document links, stored as a JSON array.
    pub source_documents: Vec<String>,
    /// The entry this one reverses, when it is a reversal.
    pub reverses: Option<String>,
    /// The lines, as storage sees them.
    pub lines: Vec<RawLine>,
}

impl RawEntry {
    /// Converts a validated [`Entry`] into the storage shape: signs
    /// every line (debit `+`, credit `−`), splits the FX legs,
    /// serialises dates to ISO strings. Amounts are narrowed to the
    /// storage width, checked.
    ///
    /// # Errors
    /// [`JournalError::Money`] when a signed minor unit does not fit
    /// an `i64`.
    pub fn from_entry(entry: &Entry) -> Result<Self, JournalError> {
        let mut lines = Vec::with_capacity(entry.lines.len());
        for line in &entry.lines {
            lines.push(RawLine::from_line(line, entry.base_currency)?);
        }
        Ok(RawEntry {
            company_id: entry.company_id.clone(),
            journal_id: entry.journal_id.clone(),
            document_date: iso_date_string(entry.document_date),
            posting_date: iso_date_string(entry.posting_date),
            tax_point_date: iso_date_string(entry.tax_point_date),
            base_currency: entry.base_currency.code().to_owned(),
            description: entry.description.clone(),
            source_documents: entry.source_documents.clone(),
            reverses: entry.reverses.clone(),
            lines,
        })
    }
}

impl RawLine {
    /// Converts one validated [`Line`]: `(side, positive amount)`
    /// becomes a signed minor unit; the FX leg splits into transaction
    /// currency, transaction minor units and the scaled rate.
    ///
    /// # Errors
    /// [`JournalError::Money`] when a signed minor unit does not fit
    /// an `i64`.
    fn from_line(line: &Line, base_currency: Currency) -> Result<Self, JournalError> {
        let narrow = |minor: i128| -> Result<i64, JournalError> {
            i64::try_from(minor)
                .map_err(|_| JournalError::Money(ledgers_money::MoneyError::Overflow))
        };
        let sign = i128::from(line.side.sign());
        let base_minor = narrow(sign * line.base.minor())?;
        let (tx_currency, tx_minor, rate_scaled) = match &line.fx {
            Some(fx) => (
                fx.transaction.currency().code().to_owned(),
                narrow(sign * fx.transaction.minor())?,
                Some(fx.rate.scaled().to_string()),
            ),
            None => (base_currency.code().to_owned(), base_minor, None),
        };
        Ok(RawLine {
            account_number: line.account_number.clone(),
            base_currency: base_currency.code().to_owned(),
            base_minor,
            tx_currency,
            tx_minor,
            rate_scaled,
            tax_code: line.tax_code.clone(),
            dimensions: line.dimensions.clone(),
            memo: line.memo.clone(),
        })
    }
}

/// What the caller supplies for one post: the identities and stamps
/// the batch cannot derive. `fiscal_year` is explicit because which
/// year a posting date belongs to is a policy decision (a year can
/// still be open after the calendar turns); `entry_id` and `outbox_id`
/// are fresh ULIDs the command layer owns; `recorded_at` is the ISO
/// 8601 UTC timestamp bound from code, never `NOW()`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PostContext {
    /// The fiscal year the number sequence is scoped to.
    pub fiscal_year: i64,
    /// A fresh ULID for the entry row.
    pub entry_id: String,
    /// The ISO 8601 UTC timestamp the entry is recorded at.
    pub recorded_at: String,
    /// A fresh ULID for the outbox event (distinct from `entry_id` so
    /// the event row is never key-collided by a future event kind on
    /// the same entry).
    pub outbox_id: String,
}

/// What a successful post hands back: the identities and the number
/// the batch assigned.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PostedEntry {
    /// The entry's id (the `PostContext` ULID).
    pub entry_id: String,
    /// The journal it posted into.
    pub journal_id: String,
    /// The fiscal year the number is scoped to.
    pub fiscal_year: i64,
    /// The gapless number the batch assigned, read back after commit.
    pub number: i64,
    /// The recorded-at timestamp.
    pub recorded_at: String,
}

/// Plans the posting batch for one raw entry (see the module docs for
/// the statement order). The only pre-batch read is the audit head —
/// which is why a retry, re-running this whole function, is correct.
/// Both periods derive and all three dates validate here, so even the
/// raw path cannot post a `vat_period` that disagrees with its tax
/// point or a malformed date.
///
/// # Errors
/// [`DbError`] when the head read fails, a date is not ISO
/// `"YYYY-MM-DD"`, or the audit event cannot be canonicalised.
#[doc(hidden)]
pub async fn plan_post_batch(
    db: &dyn Database,
    raw: &RawEntry,
    ctx: &PostContext,
    audit: &AuditContext,
) -> Result<Vec<Statement>, DbError> {
    plan_post_batch_with(db, raw, ctx, &PostOptions::default(), audit).await
}

/// [`plan_post_batch`] with the port-level [`PostOptions`] applied: the
/// period override, the person-side period guard, and the daily
/// allowance consume.
///
/// # Errors
/// As [`plan_post_batch`], plus [`DbError`] when `created_at` does not
/// start with a `YYYY-MM-DD` date and a daily allowance is requested.
#[doc(hidden)]
pub async fn plan_post_batch_with(
    db: &dyn Database,
    raw: &RawEntry,
    ctx: &PostContext,
    options: &PostOptions,
    audit: &AuditContext,
) -> Result<Vec<Statement>, DbError> {
    let head = chain::read_head(db, &raw.company_id).await?;
    let period_of = |column: &str, value: &str| -> Result<String, DbError> {
        period::period_of_date(value)
            .ok_or_else(|| DbError::Query(format!("{column} is not a YYYY-MM-DD date: {value:?}")))
    };
    let derived_period = period_of("posting_date", &raw.posting_date)?;
    let posting_period = options.period_override.clone().unwrap_or(derived_period);
    period_of("document_date", &raw.document_date)?;
    let vat_period = period_of("tax_point_date", &raw.tax_point_date)?;

    let mut statements = Vec::with_capacity(raw.lines.len() + 12);

    // 1. Gapless numbering: provision (idempotent), bump, insert
    //    reading the counter (see `entry_insert_statement`).
    statements.push(numbering::provision_statement(
        &raw.journal_id,
        ctx.fiscal_year,
    ));
    statements.push(numbering::bump_statement(&raw.journal_id, ctx.fiscal_year));
    statements.push(entry_insert_statement(raw, ctx, &vat_period));

    // 2. The lines, 1-based, in order.
    for (index, line) in raw.lines.iter().enumerate() {
        statements.push(line_insert(ctx.entry_id.as_str(), index, line));
    }

    // 3–8. The write-time checks and their releases.
    let day = options
        .daily_limit
        .as_ref()
        .map(|_| meter_day(&ctx.recorded_at))
        .transpose()?;
    statements.extend(guard_statements(raw, ctx, &posting_period, options, day));

    // 9. The audit event: a real `ledgers-audit` record of the post —
    //    who acted, the entry it touched, the input's hash — appended
    //    at head.seq + 1 in this same batch, so it exists exactly when
    //    the posting does. A stale head takes a unique violation;
    //    retry re-plans.
    statements.extend(
        append_statements(
            &raw.company_id,
            &head,
            audit,
            &AuditRecord::from_input(
                COMMAND_POST,
                &entry_input(raw),
                Outcome::Allowed,
                vec![ctx.entry_id.clone(), raw.journal_id.clone()],
                None,
            ),
            &ctx.recorded_at,
            &AuditConfig::default(),
        )
        .map_err(|err| DbError::Query(err.to_string()))?
        .statements,
    );

    // 10. The outbox enqueue: the event exists exactly when the posting
    //     commits. The payload names the entry, never its number — the
    //     number is read back after the commit, and a retried batch may
    //     take a different one; a drainer that needs it joins on
    //     entry_id.
    statements.push(
        cratefield_core::Outbox::new(OUTBOX_TABLE).enqueue_statement(
            &ctx.outbox_id,
            TOPIC_ENTRY_POSTED,
            &outbox_payload(raw, ctx, &vat_period),
            None,
            &ctx.recorded_at,
        ),
    );

    Ok(statements)
}

/// The audited command's input: the entry as storage saw it. Only the
/// hash of this is stored (`AuditRecord::from_input`), never the value —
/// an entry's description and memos are data that need not be
/// replicated into the audit log.
fn entry_input(raw: &RawEntry) -> serde_json::Value {
    serde_json::json!({
        "journalId": raw.journal_id,
        "documentDate": raw.document_date,
        "postingDate": raw.posting_date,
        "taxPointDate": raw.tax_point_date,
        "baseCurrency": raw.base_currency,
        "description": raw.description,
        "sourceDocuments": raw.source_documents,
        "reverses": raw.reverses,
        "lines": raw.lines.len(),
    })
}

/// The `journal_entry` insert: [`numbering::insert_with_number`] reads
/// the counter the same batch just advanced, so the entry's NOT NULL
/// number comes from the row in the same transaction — a refused batch
/// (any later statement aborting) rolls the bump back with it and
/// leaves no hole in the sequence.
fn entry_insert_statement(raw: &RawEntry, ctx: &PostContext, vat_period: &str) -> Statement {
    numbering::insert_with_number(
        ENTRY_TABLE,
        &[
            "id",
            "company_id",
            "journal_id",
            "fiscal_year",
            "status",
            "document_date",
            "posting_date",
            "tax_point_date",
            "vat_period",
            "recorded_at",
            "description",
            "source_documents",
            "reverses",
            "base_currency",
        ],
        vec![
            SeaValue::from(ctx.entry_id.as_str()),
            SeaValue::from(raw.company_id.as_str()),
            SeaValue::from(raw.journal_id.as_str()),
            SeaValue::BigInt(Some(ctx.fiscal_year)),
            SeaValue::from("posted"),
            SeaValue::from(raw.document_date.as_str()),
            SeaValue::from(raw.posting_date.as_str()),
            SeaValue::from(raw.tax_point_date.as_str()),
            SeaValue::from(vat_period),
            SeaValue::from(ctx.recorded_at.as_str()),
            SeaValue::from(raw.description.as_str()),
            SeaValue::from(source_documents_json(raw)),
            SeaValue::from(raw.reverses.as_deref()),
            SeaValue::from(raw.base_currency.as_str()),
        ],
        "number",
        &raw.journal_id,
        ctx.fiscal_year,
    )
}

/// The write-time checks — guard statements, never read-then-write —
/// plus their releases, in firing order: journal ownership (the
/// journal must belong to the posting company — company A cannot
/// consume company B's number sequence), the reversal target when set
/// (same company), the period-open guard (fails closed, so a closed OR
/// unknown period refuses), the accounts guard (every line's account
/// exists and is not archived), and the balanced guard (≥ 2 lines,
/// base sums to zero, a single transaction currency sums to zero; a
/// mixed-currency entry — a genuine FX trade — escapes the third
/// conjunct through `<> 1` and balances in base only, by design).
fn guard_statements(
    raw: &RawEntry,
    ctx: &PostContext,
    posting_period: &str,
    options: &PostOptions,
    day: Option<OffsetDateTime>,
) -> Vec<Statement> {
    let entry_id = ctx.entry_id.as_str();
    let company = raw.company_id.as_str();
    let owned = |key: &str, table: &str, id: &str| {
        guard::assert_statement(
            key,
            &format!("(SELECT COUNT(*) FROM {table} WHERE id = ? AND company_id = ?) = 1"),
            vec![SeaValue::from(id), SeaValue::from(company)],
        )
    };
    let journal_key = guard::guard_key(&["journal-owned", entry_id]);
    let reverses_key = guard::guard_key(&["reverses-owned", entry_id]);
    let period_key = guard::guard_key(&["period-open", company, posting_period, entry_id]);
    let accounts_key = guard::guard_key(&["accounts", entry_id]);
    let balance_key = guard::guard_key(&["balanced", entry_id]);

    let mut statements = vec![owned(&journal_key, "journal", &raw.journal_id)];
    if let Some(reverses) = raw.reverses.as_deref() {
        statements.push(owned(&reverses_key, ENTRY_TABLE, reverses));
    }
    statements.push(if options.human {
        assert_postable_statement(&period_key, company, posting_period, true)
    } else {
        period::assert_open_statement(&period_key, company, posting_period)
    });
    statements.extend([
        guard::assert_statement(
            &accounts_key,
            "(SELECT COUNT(*) FROM journal_line WHERE entry_id = ?\n\
                 AND NOT EXISTS (\n\
                 SELECT 1 FROM account acct\n\
                 WHERE acct.company_id = ? AND acct.number = journal_line.account_number\n\
                 AND acct.archived = 0)) = 0",
            vec![SeaValue::from(entry_id), SeaValue::from(company)],
        ),
        guard::assert_statement(
            &balance_key,
            "(SELECT COUNT(*) FROM journal_line WHERE entry_id = ?) >= 2\n\
                 AND (SELECT COALESCE(SUM(base_minor), 0) FROM journal_line WHERE entry_id = ?) = 0\n\
                 AND ((SELECT COUNT(DISTINCT tx_currency) FROM journal_line WHERE entry_id = ?) <> 1\n\
                 OR (SELECT COALESCE(SUM(tx_minor), 0) FROM journal_line WHERE entry_id = ?) = 0)",
            vec![
                SeaValue::from(entry_id),
                SeaValue::from(entry_id),
                SeaValue::from(entry_id),
                SeaValue::from(entry_id),
            ],
        ),
    ]);
    // The daily allowance: the usage meter's guarded upsert, beside the
    // work it pays for; a spent allowance fails NOT NULL in this batch.
    if let (Some((subject, limit)), Some(day)) = (&options.daily_limit, day) {
        statements.push(limit::daily_consume_statement(
            subject, METER_POST, day, 1, *limit,
        ));
    }
    statements.extend([
        guard::release_statement(&journal_key),
        guard::release_statement(&reverses_key),
        guard::release_statement(&period_key),
        guard::release_statement(&accounts_key),
        guard::release_statement(&balance_key),
    ]);
    statements
}

/// Posts one validated entry: converts it to storage shape, plans and
/// submits the batch under `policy` (retrying — re-planning — on
/// unique-violation races), then reads the assigned number back.
/// `sleep` is the caller's back-off; the library itself never sleeps.
///
/// This is the only production path to a posted entry. There is no
/// update and no delete — not here, not anywhere in this crate.
///
/// # Errors
/// [`JournalError::Conflict`] when the retry budget ran out on races;
/// the guard refusals mapped to their domain variants
/// ([`JournalError::PeriodNotOpen`], [`JournalError::UnknownAccount`],
/// [`JournalError::ForeignJournal`], [`JournalError::ForeignReversal`]);
/// [`JournalError::Database`] for anything the diagnostics could not
/// attribute; [`JournalError::Money`] when an amount does not fit the
/// storage width during conversion.
pub async fn post_entry(
    db: &dyn Database,
    entry: &Entry,
    ctx: &PostContext,
    policy: RetryPolicy,
    sleep: impl FnMut(Duration),
    audit: &AuditContext,
) -> Result<PostedEntry, JournalError> {
    let raw = RawEntry::from_entry(entry)?;
    post_raw_entry(db, &raw, ctx, policy, sleep, audit).await
}

/// Posts one **raw** entry: the same batch, no validation in front —
/// the test path that proves the SQL guards refuse bad rows alone.
/// Production code uses [`post_entry`].
///
/// # Errors
/// As [`post_entry`]; an unattributable refusal stays
/// [`JournalError::Database`].
#[doc(hidden)]
pub async fn post_raw_entry(
    db: &dyn Database,
    raw: &RawEntry,
    ctx: &PostContext,
    policy: RetryPolicy,
    sleep: impl FnMut(Duration),
    audit: &AuditContext,
) -> Result<PostedEntry, JournalError> {
    post_raw_entry_with(db, raw, ctx, policy, sleep, &PostOptions::default(), audit).await
}

/// [`post_raw_entry`] with the port-level [`PostOptions`]: the same
/// batch, with the period override, person-side guard and daily
/// allowance applied. The refusal attribution honours the same options.
///
/// # Errors
/// As [`post_entry`].
#[doc(hidden)]
pub async fn post_raw_entry_with(
    db: &dyn Database,
    raw: &RawEntry,
    ctx: &PostContext,
    policy: RetryPolicy,
    sleep: impl FnMut(Duration),
    options: &PostOptions,
    audit: &AuditContext,
) -> Result<PostedEntry, JournalError> {
    if let Err(err) = retry::run_batch_with_retry(
        db,
        policy,
        || plan_post_batch_with(db, raw, ctx, options, audit),
        sleep,
    )
    .await
    {
        return Err(match err {
            ledgers_db::GuardError::Conflict { attempts } => JournalError::Conflict { attempts },
            ledgers_db::GuardError::Database(db_error) if is_guard_refusal(&db_error) => {
                // Reads after a failed write (ADR 0004): the batch is
                // gone; these only choose which error the caller reads.
                diagnose_refusal(db, raw, options)
                    .await
                    .unwrap_or(JournalError::Database(db_error))
            }
            ledgers_db::GuardError::Database(db_error) => JournalError::Database(db_error),
        });
    }
    let number = read_number(db, &ctx.entry_id).await?;
    Ok(PostedEntry {
        entry_id: ctx.entry_id.clone(),
        journal_id: raw.journal_id.clone(),
        fiscal_year: ctx.fiscal_year,
        number,
        recorded_at: ctx.recorded_at.clone(),
    })
}

/// Whether a failed statement was a guard probe's `NOT NULL` refusal
/// (the probe row's `ok` column takes NULL exactly when the invariant
/// does not hold) — as opposed to an engine failure on any other
/// statement. Both dialects name the table and the constraint in the
/// message.
fn is_guard_refusal(err: &DbError) -> bool {
    let text = err.to_string().to_ascii_lowercase();
    text.contains("ledger_guard") && text.contains("null")
}

/// Attributes a guard refusal to its domain error with cheap reads:
/// journal ownership, reversal-target ownership, the posting period's
/// status, and each line's account against the chart. `None` when
/// nothing attributable is found (an unbalanced raw entry, or a read
/// that itself failed) — the caller then keeps the original database
/// error, which is the honest answer for both.
async fn diagnose_refusal(
    db: &dyn Database,
    raw: &RawEntry,
    options: &PostOptions,
) -> Option<JournalError> {
    async fn scalar(db: &dyn Database, statement: Statement) -> Option<i64> {
        let rows = db.query(&statement).await.ok()?;
        rows.first().and_then(|row| row.get::<i64>("n"))
    }
    let company = raw.company_id.as_str();
    let count_of = |sql: String, id: &str| -> _ {
        Statement::with_values(sql, vec![SeaValue::from(id), SeaValue::from(company)])
    };

    if scalar(
        db,
        count_of(
            "SELECT COUNT(*) AS n FROM journal WHERE id = ? AND company_id = ?".to_owned(),
            &raw.journal_id,
        ),
    )
    .await?
        == 0
    {
        return Some(JournalError::ForeignJournal {
            journal: raw.journal_id.clone(),
        });
    }
    if let Some(reverses) = raw.reverses.as_deref()
        && scalar(
            db,
            count_of(
                "SELECT COUNT(*) AS n FROM journal_entry WHERE id = ? AND company_id = ?"
                    .to_owned(),
                reverses,
            ),
        )
        .await?
            == 0
    {
        return Some(JournalError::ForeignReversal {
            entry: reverses.to_owned(),
        });
    }
    let posting_period = options
        .period_override
        .clone()
        .or_else(|| period::period_of_date(&raw.posting_date));
    if let Some(posting_period) = posting_period {
        let rows = db
            .query(&Statement::with_values(
                "SELECT status FROM accounting_period\n\
                 WHERE company_id = ? AND period = ?"
                    .to_owned(),
                vec![
                    SeaValue::from(company),
                    SeaValue::from(posting_period.as_str()),
                ],
            ))
            .await
            .ok()?;
        let open = rows
            .first()
            .and_then(|row| row.get::<String>("status"))
            .is_some_and(|status| status == "open" || (options.human && status == "soft-closed"));
        if !open {
            return Some(JournalError::PeriodNotOpen {
                period: posting_period,
            });
        }
    }
    for line in &raw.lines {
        let present = scalar(
            db,
            Statement::with_values(
                "SELECT COUNT(*) AS n FROM account\n\
                     WHERE company_id = ? AND number = ? AND archived = 0"
                    .to_owned(),
                vec![
                    SeaValue::from(company),
                    SeaValue::from(line.account_number.as_str()),
                ],
            ),
        )
        .await?;
        if present == 0 {
            return Some(JournalError::UnknownAccount {
                account: line.account_number.clone(),
            });
        }
    }
    None
}

/// The usage-meter day of a post: the UTC day it is made (`created_at`),
/// never the accounting date — the same rule `ledgers_db::suite` applies,
/// so an agent cannot pick a fresh allowance by backdating.
fn meter_day(created_at: &str) -> Result<OffsetDateTime, DbError> {
    let bad = || {
        DbError::Query(format!(
            "created_at {created_at} does not start with a YYYY-MM-DD date"
        ))
    };
    let date = created_at
        .get(..10)
        .filter(|date| period::period_of_date(date).is_some())
        .ok_or_else(bad)?;
    let year: i32 = date[0..4].parse().map_err(|_| bad())?;
    let month: u8 = date[5..7].parse().map_err(|_| bad())?;
    let day: u8 = date[8..10].parse().map_err(|_| bad())?;
    let month = Month::try_from(month).map_err(|_| bad())?;
    let calendar = Date::from_calendar_date(year, month, day).map_err(|_| bad())?;
    Ok(calendar.midnight().assume_utc())
}

/// The number the batch assigned, read back after commit. A missing
/// row here means the committed entry is not readable — an engine
/// failure, not a domain condition.
async fn read_number(db: &dyn Database, entry_id: &str) -> Result<i64, JournalError> {
    let rows = db
        .query(&Statement::with_values(
            format!("SELECT number FROM {ENTRY_TABLE} WHERE id = ?"),
            vec![SeaValue::from(entry_id)],
        ))
        .await?;
    rows.first()
        .and_then(|row| row.get::<i64>("number"))
        .ok_or_else(|| {
            JournalError::Database(DbError::Query(format!(
                "posted entry {entry_id} has no readable number"
            )))
        })
}

/// The INSERT for one line: `(entry_id, line_no, …)`, signed minor
/// units, JSON dimensions.
fn line_insert(entry_id: &str, index: usize, line: &RawLine) -> Statement {
    Statement::with_values(
        format!(
            "INSERT INTO {LINE_TABLE} (\n\
             entry_id, line_no, account_number, base_currency, base_minor,\n\
             tx_currency, tx_minor, rate_scaled, tax_code, dimensions, memo\n\
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        ),
        vec![
            SeaValue::from(entry_id),
            SeaValue::BigInt(Some(i64::try_from(index + 1).expect("a small line count"))),
            SeaValue::from(line.account_number.as_str()),
            SeaValue::from(line.base_currency.as_str()),
            SeaValue::BigInt(Some(line.base_minor)),
            SeaValue::from(line.tx_currency.as_str()),
            SeaValue::BigInt(Some(line.tx_minor)),
            SeaValue::from(line.rate_scaled.as_deref()),
            SeaValue::from(line.tax_code.as_deref()),
            SeaValue::from(dimensions_json(line)),
            SeaValue::from(line.memo.as_deref()),
        ],
    )
}

/// The source-documents column value: a JSON array of strings.
fn source_documents_json(raw: &RawEntry) -> String {
    serde_json::to_string(&raw.source_documents).unwrap_or_else(|_| "[]".to_owned())
}

/// The dimensions column value: a JSON object, `{}` when empty.
fn dimensions_json(line: &RawLine) -> String {
    serde_json::to_string(&line.dimensions).unwrap_or_else(|_| "{}".to_owned())
}

/// The outbox event payload: the identities a subscriber needs,
/// without the number (see the enqueue site for why). Serialisation
/// of strings into a JSON object cannot fail; the fallback keeps the
/// signature total anyway.
fn outbox_payload(raw: &RawEntry, ctx: &PostContext, vat_period: &str) -> String {
    serde_json::json!({
        "companyId": raw.company_id,
        "journalId": raw.journal_id,
        "entryId": ctx.entry_id,
        "fiscalYear": ctx.fiscal_year,
        "vatPeriod": vat_period,
        "reverses": raw.reverses,
        "postedAt": ctx.recorded_at,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::{EntryDraft, Fx, Side};
    use ledgers_money::{Amount, Rate};
    use time::Date;

    fn eur() -> Currency {
        Currency::from_code("EUR").expect("EUR is in the table")
    }
    fn usd() -> Currency {
        Currency::from_code("USD").expect("USD is in the table")
    }
    fn date() -> Date {
        Date::from_calendar_date(2026, time::Month::April, 10).expect("a calendar date")
    }

    fn validated_entry() -> Entry {
        let mut debit = Line {
            account_number: "01000".to_owned(),
            side: Side::Debit,
            base: Amount::new(9000, eur()),
            fx: None,
            tax_code: None,
            dimensions: BTreeMap::new(),
            memo: Some("pay the bill".to_owned()),
        };
        debit.fx = Some(Fx {
            transaction: Amount::new(10_000, usd()),
            rate: Rate::parse("0.9").expect("a rate"),
        });
        let credit = Line {
            account_number: "04000".to_owned(),
            side: Side::Credit,
            base: Amount::new(9000, eur()),
            fx: None,
            tax_code: Some("19-U".to_owned()),
            dimensions: BTreeMap::from([("cost-centre".to_owned(), "cc-1".to_owned())]),
            memo: None,
        };
        Entry::from_draft(
            EntryDraft {
                journal_id: "01JOURNAL0000000000000000000".to_owned(),
                document_date: date(),
                posting_date: date(),
                tax_point_date: None,
                base_currency: eur(),
                description: "Pay USD bill".to_owned(),
                source_documents: vec!["bill:B-42".to_owned()],
                reverses: None,
                lines: vec![debit, credit],
            },
            "01COMPANY000000000000000000",
        )
        .expect("the fixture validates")
    }

    #[test]
    fn a_validated_entry_converts_to_storage_shape() {
        let raw = RawEntry::from_entry(&validated_entry()).expect("conversion");
        assert_eq!(raw.base_currency, "EUR");
        assert_eq!(raw.posting_date, "2026-04-10");
        let debit = &raw.lines[0];
        assert_eq!(debit.base_minor, 9000);
        assert_eq!(debit.tx_currency, "USD");
        assert_eq!(debit.tx_minor, 10_000);
        assert_eq!(debit.rate_scaled.as_deref(), Some("900000000000"));
        let credit = &raw.lines[1];
        assert_eq!(credit.base_minor, -9000);
        assert_eq!(credit.tx_currency, "EUR");
        assert_eq!(credit.tx_minor, -9000);
        assert_eq!(credit.rate_scaled, None);
    }

    #[test]
    fn the_outbox_payload_names_the_entry_not_the_number() {
        let raw = RawEntry::from_entry(&validated_entry()).expect("conversion");
        let ctx = PostContext {
            fiscal_year: 2026,
            entry_id: "01ENTRY000000000000000000000".to_owned(),
            recorded_at: "2026-04-10T10:00:00Z".to_owned(),
            outbox_id: "01OUTBOX0000000000000000000".to_owned(),
        };
        let payload: serde_json::Value =
            serde_json::from_str(&outbox_payload(&raw, &ctx, "2026-04")).expect("json");
        assert_eq!(payload["entryId"], "01ENTRY000000000000000000000");
        assert_eq!(payload["companyId"], "01COMPANY000000000000000000");
        assert_eq!(payload["fiscalYear"], 2026);
        assert_eq!(payload["vatPeriod"], "2026-04");
        assert_eq!(payload["postedAt"], "2026-04-10T10:00:00Z");
        assert!(
            payload.get("number").is_none(),
            "the payload must not carry the number"
        );
    }

    #[test]
    fn json_columns_serialise_stably() {
        let raw = RawEntry::from_entry(&validated_entry()).expect("conversion");
        assert_eq!(source_documents_json(&raw), r#"["bill:B-42"]"#);
        assert_eq!(dimensions_json(&raw.lines[1]), r#"{"cost-centre":"cc-1"}"#);
        assert_eq!(dimensions_json(&raw.lines[0]), "{}");
    }

    #[test]
    fn guard_refusals_are_recognised_by_their_statement_text() {
        for refused in [
            DbError::Query("NOT NULL constraint failed: ledger_guard.ok".to_owned()),
            DbError::Query(
                "null value in column \"ok\" of relation \"ledger_guard\" violates not-null \
                 constraint"
                    .to_owned(),
            ),
        ] {
            assert!(is_guard_refusal(&refused), "{refused}");
        }
        for other in [
            DbError::Query("database is locked".to_owned()),
            DbError::Query("NOT NULL constraint failed: journal_line.entry_id".to_owned()),
        ] {
            assert!(!is_guard_refusal(&other), "{other}");
        }
    }
}
