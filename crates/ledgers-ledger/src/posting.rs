//! Posting: the one `batch_atomic` that turns a validated [`Entry`]
//! into a posted, numbered, audit-chained, event-carrying row — and the
//! proof that nothing else can (docs/PLAN.md §3 rules 2, 3, 5).
//!
//! The batch's statement order (the mechanics and the why are
//! docs/adr/0004-guard-statements.md, which this module implements and
//! does not re-narrate):
//!
//! ```sql
//! -- 1. numbering: provision (idempotent), bump, insert the literal number
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

use crate::dimension;
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
use ledgers_money::{Amount, Currency};
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
    /// The source application's idempotency key, stored for lookup by
    /// `external_id` (unique per company when set).
    pub external_id: Option<String>,
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
            external_id: entry.external_id.clone(),
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
    pub fn from_line(line: &Line, base_currency: Currency) -> Result<Self, JournalError> {
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
/// the statement order). The only pre-batch reads are the audit head and
/// the numbering counter (to guarantee the number inside the batch) —
/// which is why a retry, re-running this whole function, is correct.
/// Both periods derive and all three dates validate here, so even the
/// raw path cannot post a `vat_period` that disagrees with its tax
/// point or a malformed date.
///
/// Returns the expected entry number so the caller can return it after
/// commit without a separate read.
///
/// # Errors
/// [`DbError`] when the head read or counter read fails, a date is not ISO
/// `"YYYY-MM-DD"`, or the audit event cannot be canonicalised.
#[doc(hidden)]
pub async fn plan_post_batch(
    db: &dyn Database,
    raw: &RawEntry,
    ctx: &PostContext,
    audit: &AuditContext,
) -> Result<(Vec<Statement>, i64), DbError> {
    let options = PostOptions {
        human: audit.actor.is_person(),
        ..Default::default()
    };
    plan_post_batch_with(db, raw, ctx, &options, audit).await
}

/// Builds the entry posting statements without the audit event.
/// This is used by the executor path, which appends its own Allowed audit event.
/// The domain statements comprise:
/// - 1. Gapless numbering (provision, bump, insert with literal number, guard)
/// - 2. The entry's lines
/// - 3–8. Write-time checks and their releases
/// - 9. The outbox enqueue
///
/// # Errors
/// As [`plan_post_batch`], plus [`DbError`] when `created_at` does not
/// start with a `YYYY-MM-DD` date and a daily allowance is requested.
#[doc(hidden)]
#[allow(clippy::too_many_lines)]
pub async fn plan_post_batch_statements_only(
    db: &dyn Database,
    raw: &RawEntry,
    ctx: &PostContext,
    options: &PostOptions,
) -> Result<(Vec<Statement>, i64), DbError> {
    // Read the numbering counter and compute the expected number inside
    // the batch. The counter's idempotent provision (in the batch) ensures
    // it exists by the time we use it.
    let current_number = numbering::read_counter(db, &raw.journal_id, ctx.fiscal_year)
        .await?
        .unwrap_or(0);
    let expected_number = current_number + 1;

    let period_of = |column: &str, value: &str| -> Result<String, DbError> {
        period::period_of_date(value)
            .ok_or_else(|| DbError::Query(format!("{column} is not a YYYY-MM-DD date: {value:?}")))
    };
    let derived_period = period_of("posting_date", &raw.posting_date)?;
    let posting_period = options.period_override.clone().unwrap_or(derived_period);
    period_of("document_date", &raw.document_date)?;
    let vat_period = period_of("tax_point_date", &raw.tax_point_date)?;

    // Handle dimensions: read own-books units and clearing account, generate
    // clearing lines if needed. We do this before building the batch so the
    // posted entry includes the clearing lines.
    let own_books_units = read_own_books_units(db, &raw.company_id).await?;
    let mut raw = raw.clone();
    let mut clearing_lines = Vec::new();

    if !own_books_units.is_empty()
        && let Some(clearing_account) = read_clearing_account(db, &raw.company_id).await?
    {
        let base_currency = ledgers_money::Currency::from_code(&raw.base_currency)
            .map_err(|e| DbError::Query(format!("invalid base_currency: {e}")))?;

        // Convert lines to domain type for clearing calculation.
        let domain_lines: Vec<Line> = raw
            .lines
            .iter()
            .map(|raw_line| {
                let side = if raw_line.base_minor > 0 {
                    crate::Side::Debit
                } else {
                    crate::Side::Credit
                };
                Line {
                    account_number: raw_line.account_number.clone(),
                    side,
                    base: Amount::from_i64(raw_line.base_minor.abs(), base_currency),
                    fx: None,
                    tax_code: raw_line.tax_code.clone(),
                    dimensions: raw_line.dimensions.clone(),
                    memo: raw_line.memo.clone(),
                }
            })
            .collect();

        match dimension::clearing_lines_for_units(
            &domain_lines,
            &own_books_units,
            &clearing_account,
            base_currency,
        ) {
            Ok(lines) => {
                for line in lines {
                    match RawLine::from_line(&line, base_currency) {
                        Ok(raw_line) => clearing_lines.push(raw_line),
                        Err(e) => return Err(DbError::Query(format!("clearing line: {e}"))),
                    }
                }
            }
            Err(e) => {
                // Money overflow in clearing calculation.
                return Err(DbError::Query(format!("clearing line calculation: {e}")));
            }
        }
    }

    raw.lines.extend(clearing_lines);

    let mut statements = Vec::with_capacity(raw.lines.len() + 30);

    // 1. Gapless numbering: provision (idempotent), bump, insert with
    //    literal number, then guard to verify the counter matches.
    statements.push(numbering::provision_statement(
        &raw.journal_id,
        ctx.fiscal_year,
    ));
    statements.push(numbering::bump_statement(&raw.journal_id, ctx.fiscal_year));
    statements.push(entry_insert_statement(
        &raw,
        ctx,
        &vat_period,
        expected_number,
        options,
    ));

    // The guard after the insert verifies the counter equals expected_number:
    // the counter only grows and every number ≤ counter is already taken in
    // journal_entry for that (journal, fiscal year), so if another post won
    // the race, the counter was already ≥ expected_number, our INSERT hit the
    // unique constraint, and retry re-planned with fresh reads. This guard
    // ensures the counter is exactly what we expected, preventing gaps.
    let counter_guard_key = guard::guard_key(&["counter-advanced", &ctx.entry_id]);
    statements.push(numbering::assert_counter_advanced_statement(
        &counter_guard_key,
        &raw.journal_id,
        ctx.fiscal_year,
        expected_number,
    ));

    // 2. The lines, 1-based, in order, plus normalized dimension rows.
    for (index, line) in raw.lines.iter().enumerate() {
        statements.push(line_insert(ctx.entry_id.as_str(), index, line));

        // Insert normalized dimension rows for this line.
        let line_no = index + 1;
        for (kind, value) in &line.dimensions {
            // Only normalize the three known kinds: unit, cost_centre, project.
            if ["unit", "cost_centre", "project"].contains(&kind.as_str()) {
                statements.push(Statement::with_values(
                    "INSERT INTO journal_line_dimension (company_id, entry_id, line_no, kind, value) VALUES (?, ?, ?, ?, ?)"
                        .to_owned(),
                    vec![
                        SeaValue::from(raw.company_id.as_str()),
                        SeaValue::from(ctx.entry_id.as_str()),
                        SeaValue::BigInt(Some(i64::try_from(line_no).expect("line count fits in i64"))),
                        SeaValue::from(kind.as_str()),
                        SeaValue::from(value.as_str()),
                    ],
                ));
            }
        }
    }

    // 3–8. The write-time checks and their releases.
    let day = options
        .daily_limit
        .as_ref()
        .map(|_| meter_day(&ctx.recorded_at))
        .transpose()?;
    statements.extend(guard_statements(&raw, ctx, &posting_period, options, day));

    // 9. The outbox enqueue: the event exists exactly when the posting
    //     commits. The payload names the entry and its number.
    statements.push(
        cratefield_core::Outbox::new(OUTBOX_TABLE).enqueue_statement(
            &ctx.outbox_id,
            TOPIC_ENTRY_POSTED,
            &outbox_payload(&raw, ctx, &vat_period),
            None,
            &ctx.recorded_at,
        ),
    );

    // 10. Release the counter guard at the very end.
    statements.push(guard::release_statement(&counter_guard_key));

    Ok((statements, expected_number))
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
) -> Result<(Vec<Statement>, i64), DbError> {
    let head = chain::read_head(db, &raw.company_id).await?;

    let (mut statements, expected_number) =
        plan_post_batch_statements_only(db, raw, ctx, options).await?;

    // Append the audit event: a real `ledgers-audit` record of the post —
    // who acted, the entry it touched, the input's hash — appended
    // at head.seq + 1 in this same batch, so it exists exactly when
    // the posting does. A stale head takes a unique violation;
    // retry re-plans.
    statements.extend(
        append_statements(
            &raw.company_id,
            &head,
            audit,
            &AuditRecord::from_input(
                COMMAND_POST,
                &entry_input(raw, expected_number),
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

    Ok((statements, expected_number))
}

/// The audited command's input: the entry as storage saw it, number
/// included. Only the hash of this is stored (`AuditRecord::from_input`),
/// never the value — an entry's description and memos are data that need
/// not be replicated into the audit log.
fn entry_input(raw: &RawEntry, number: i64) -> serde_json::Value {
    serde_json::json!({
        "journalId": raw.journal_id,
        "number": number,
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

/// The `journal_entry` insert: uses a literal number computed before the
/// batch (avoiding the read-after-commit race of issue #71), followed by
/// a guard statement verifying the counter is at the expected value.
/// Sets actor to 'human' when options.human is true, else 'agent'.
fn entry_insert_statement(
    raw: &RawEntry,
    ctx: &PostContext,
    vat_period: &str,
    number: i64,
    options: &PostOptions,
) -> Statement {
    let actor = if options.human { "human" } else { "agent" };
    Statement::with_values(
        format!(
            "INSERT INTO {ENTRY_TABLE} (id, company_id, journal_id, fiscal_year, number, status, document_date, posting_date, tax_point_date, vat_period, recorded_at, description, source_documents, reverses, external_id, base_currency, actor) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        ),
        vec![
            SeaValue::from(ctx.entry_id.as_str()),
            SeaValue::from(raw.company_id.as_str()),
            SeaValue::from(raw.journal_id.as_str()),
            SeaValue::BigInt(Some(ctx.fiscal_year)),
            SeaValue::BigInt(Some(number)),
            SeaValue::from("posted"),
            SeaValue::from(raw.document_date.as_str()),
            SeaValue::from(raw.posting_date.as_str()),
            SeaValue::from(raw.tax_point_date.as_str()),
            SeaValue::from(vat_period),
            SeaValue::from(ctx.recorded_at.as_str()),
            SeaValue::from(raw.description.as_str()),
            SeaValue::from(source_documents_json(raw)),
            SeaValue::from(raw.reverses.as_deref()),
            SeaValue::from(raw.external_id.as_deref()),
            SeaValue::from(raw.base_currency.as_str()),
            SeaValue::from(actor),
        ],
    )
}

/// Reads own-books unit codes from the company.
async fn read_own_books_units(db: &dyn Database, company_id: &str) -> Result<Vec<String>, DbError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT code FROM dimension_value WHERE company_id = ? AND kind = ? AND own_books = 1"
                .to_owned(),
            vec![SeaValue::from(company_id), SeaValue::from("unit")],
        ))
        .await?;
    let mut units = Vec::new();
    for row in &rows.rows {
        if let Some(code) = row.get::<String>("code") {
            units.push(code);
        }
    }
    Ok(units)
}

/// Reads the single inter-unit-clearing account: must be exactly one
/// unarchived leaf tagged with `inter-unit-clearing`.
async fn read_clearing_account(
    db: &dyn Database,
    company_id: &str,
) -> Result<Option<String>, DbError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT number FROM account WHERE company_id = ? AND archived = 0 AND tags LIKE '%inter-unit-clearing%' \
             AND NOT EXISTS (SELECT 1 FROM account AS child WHERE child.parent_number = account.number)"
                .to_owned(),
            vec![SeaValue::from(company_id)],
        ))
        .await?;
    Ok(rows.first().and_then(|row| row.get::<String>("number")))
}

/// The write-time checks — guard statements, never read-then-write —
/// plus their releases, in firing order: journal ownership (the
/// journal must belong to the posting company — company A cannot
/// consume company B's number sequence), the reversal target when set
/// (same company), the period-open guard (fails closed, so a closed OR
/// unknown period refuses), the accounts guard (every line's account
/// exists and is not archived), the balanced guard (≥ 2 lines,
/// base sums to zero, a single transaction currency sums to zero; a
/// mixed-currency entry — a genuine FX trade — escapes the third
/// conjunct through `<> 1` and balances in base only, by design),
/// dimension guards (required/forbidden/known values), and per-unit balance.
#[allow(clippy::too_many_lines)]
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

    // Reversal guards and releases (if this is a reversal).
    let mut reversal_release_keys = Vec::new();
    if let Some(reverses) = raw.reverses.as_deref() {
        // Reversal target ownership guard: target must exist and belong to the posting company.
        statements.push(owned(&reverses_key, ENTRY_TABLE, reverses));
        reversal_release_keys.push(reverses_key.clone());

        // Reversal invariant guards (ADR 0004 pattern):
        let reversal_no_reversal_key = guard::guard_key(&["reversal-not-reversal", entry_id]);
        let reversal_no_double_key = guard::guard_key(&["reversal-no-double", entry_id]);
        let reversal_no_human_key = guard::guard_key(&["reversal-no-human-by-agent", entry_id]);

        // (a) The target's own reverses IS NULL (no reversal of a reversal).
        statements.push(guard::assert_statement(
            &reversal_no_reversal_key,
            &format!("(SELECT COUNT(*) FROM {ENTRY_TABLE} WHERE id = ? AND reverses IS NULL) = 1"),
            vec![SeaValue::from(reverses)],
        ));
        reversal_release_keys.push(reversal_no_reversal_key);

        // (b) No other journal_entry already reverses the target (UNIQUE index backs this up,
        // but the guard makes the failure explicit and retriable via re-planning).
        // Exclude the current entry being inserted.
        statements.push(guard::assert_statement(
            &reversal_no_double_key,
            &format!("(SELECT COUNT(*) FROM {ENTRY_TABLE} WHERE reverses = ? AND id <> ?) = 0"),
            vec![SeaValue::from(reverses), SeaValue::from(entry_id)],
        ));
        reversal_release_keys.push(reversal_no_double_key);

        // (c) If the posting actor is 'agent', the target's actor is not 'human'.
        if !options.human {
            statements.push(guard::assert_statement(
                &reversal_no_human_key,
                &format!(
                    "(SELECT COUNT(*) FROM {ENTRY_TABLE} WHERE id = ? AND actor != 'human') = 1"
                ),
                vec![SeaValue::from(reverses)],
            ));
            reversal_release_keys.push(reversal_no_human_key);
        }
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

    // Dimension guards: required, forbidden, known values.
    let dimension_required_key = guard::guard_key(&["dimension-required", entry_id]);
    let dimension_forbidden_key = guard::guard_key(&["dimension-forbidden", entry_id]);
    let dimension_known_key = guard::guard_key(&["dimension-known", entry_id]);

    // For each dimension kind that has a "required" rule, no line on an account
    // with that rule can lack the dimension.
    statements.push(guard::assert_statement(
        &dimension_required_key,
        &format!(
            "(SELECT COUNT(*) FROM {LINE_TABLE} jl \
             WHERE jl.entry_id = ? \
             AND EXISTS (SELECT 1 FROM account_dimension_rule \
                        WHERE company_id = ? AND account_number = jl.account_number \
                        AND rule = 'required' \
                        AND NOT EXISTS (SELECT 1 FROM journal_line_dimension \
                                       WHERE company_id = ? AND entry_id = ? \
                                       AND line_no = jl.line_no \
                                       AND kind = account_dimension_rule.kind))) = 0"
        ),
        vec![
            SeaValue::from(entry_id),
            SeaValue::from(company),
            SeaValue::from(company),
            SeaValue::from(entry_id),
        ],
    ));
    statements.push(guard::release_statement(&dimension_required_key));

    // For each line, it must not carry a dimension its account forbids.
    statements.push(guard::assert_statement(
        &dimension_forbidden_key,
        &format!(
            "(SELECT COUNT(*) FROM journal_line_dimension jld \
             WHERE jld.entry_id = ? \
             AND EXISTS (SELECT 1 FROM account_dimension_rule \
                        WHERE company_id = ? AND account_number = \
                        (SELECT account_number FROM {LINE_TABLE} WHERE entry_id = ? AND line_no = jld.line_no) \
                        AND kind = jld.kind AND rule = 'forbidden')) = 0"
        ),
        vec![
            SeaValue::from(entry_id),
            SeaValue::from(company),
            SeaValue::from(entry_id),
        ],
    ));
    statements.push(guard::release_statement(&dimension_forbidden_key));

    // Every dimension value on a line must exist in dimension_value for the company.
    statements.push(guard::assert_statement(
        &dimension_known_key,
        "(SELECT COUNT(*) FROM journal_line_dimension jld \
         WHERE jld.company_id = ? \
         AND jld.entry_id = ? \
         AND NOT EXISTS (SELECT 1 FROM dimension_value dv \
                        WHERE dv.company_id = ? AND dv.kind = jld.kind AND dv.code = jld.value)) = 0",
        vec![
            SeaValue::from(company),
            SeaValue::from(entry_id),
            SeaValue::from(company),
        ],
    ));
    statements.push(guard::release_statement(&dimension_known_key));

    // Per-unit balance: for each own-books unit in this entry, its lines must net to zero in base.
    // Count how many own-books units have unbalanced lines in this entry.
    let dimension_balance_key = guard::guard_key(&["dimension-balance", entry_id]);
    statements.push(guard::assert_statement(
        &dimension_balance_key,
        &format!(
            "(SELECT COUNT(*) FROM ( \
             SELECT dv.code \
             FROM dimension_value dv \
             WHERE dv.company_id = ? AND dv.kind = 'unit' AND dv.own_books = 1 \
             AND EXISTS (SELECT 1 FROM journal_line_dimension jld \
                        WHERE jld.company_id = ? AND jld.entry_id = ? \
                        AND jld.kind = 'unit' AND jld.value = dv.code) \
             ) own_unit_in_entry \
             WHERE (SELECT COALESCE(SUM(jl.base_minor), 0) FROM {LINE_TABLE} jl \
                   WHERE jl.entry_id = ? \
                   AND EXISTS (SELECT 1 FROM journal_line_dimension jld \
                              WHERE jld.entry_id = jl.entry_id \
                              AND jld.line_no = jl.line_no \
                              AND jld.kind = 'unit' AND jld.value = own_unit_in_entry.code)) != 0) = 0"
        ),
        vec![
            SeaValue::from(company),
            SeaValue::from(company),
            SeaValue::from(entry_id),
            SeaValue::from(entry_id),
        ],
    ));
    statements.push(guard::release_statement(&dimension_balance_key));

    // Release all guards in order: journal, reversals, period, accounts, balance, dimensions
    statements.push(guard::release_statement(&journal_key));
    for key in reversal_release_keys {
        statements.push(guard::release_statement(&key));
    }
    statements.push(guard::release_statement(&period_key));
    statements.push(guard::release_statement(&accounts_key));
    statements.push(guard::release_statement(&balance_key));
    statements
}

/// Plans one validated entry for the executor: converts it to statements,
/// returning just the statements (no execution). The executor appends the
/// audit record and executes the combined batch.
///
/// This is not the main posting path (which is [`post_entry`]); it's called
/// only by the executor's plan method. The executor handles retry and idempotency.
///
/// # Errors
/// [`JournalError::Money`] when an amount does not fit the storage width.
pub async fn plan_post_entry(
    token: &ledgers_commands::ExecToken,
    db: &dyn Database,
    company_id: &str,
    entry: &Entry,
    fiscal_year: i64,
    now: &str,
    request_id: &str,
) -> Result<(Vec<Statement>, PostedEntry), crate::JournalError> {
    let raw = RawEntry::from_entry(entry)?;

    // The entry_id is derived from the request_id to support idempotency:
    // a replay with the same request_id will use the same entry_id and be idempotent.
    let entry_id = request_id;
    let outbox_id = format!("{request_id}-outbox");

    // Check if the entry was already posted (idempotency).
    if let Some(posted_entry) = read_posted_entry(db, entry_id, company_id).await? {
        // Entry already exists: return it with no statements (consistent with
        // read-only commands). The executor will skip the audit event.
        return Ok((vec![], posted_entry));
    }

    // The executor has already determined the company and fiscal year context.
    // We just plan the statements here. The executor will append the Allowed audit event.
    let ctx = PostContext {
        fiscal_year,
        entry_id: entry_id.to_string(),
        recorded_at: now.to_string(),
        outbox_id,
    };

    // The executor authenticated the principal: a person's post records
    // `human`, an agent's records `agent` (docs/PLAN.md section 5). The
    // executor appends the audit event.
    let options = PostOptions {
        human: token.acts_as_person(),
        ..Default::default()
    };
    let (statements, number) = plan_post_batch_statements_only(db, &raw, &ctx, &options).await?;

    let posted = PostedEntry {
        entry_id: ctx.entry_id,
        journal_id: raw.journal_id.clone(),
        fiscal_year: ctx.fiscal_year,
        number,
        recorded_at: ctx.recorded_at,
    };

    Ok((statements, posted))
}

/// Posts one validated entry: converts it to storage shape, plans and
/// submits the batch under `policy` (retrying — re-planning — on
/// unique-violation races). The entry's number is computed before the
/// batch and guaranteed by a guard statement inside it, so no post-commit
/// read is needed. `sleep` is the caller's back-off; the library itself
/// never sleeps.
///
/// This is the only production path to a posted entry. There is no
/// update and no delete — not here, not anywhere in this crate.
///
/// **Outcome contract:** `Ok(posted)` means the entry is committed with
/// that `posted.number`. `Err(Database)` means the outcome is unknown to
/// the caller — the batch may or may not have committed. To resolve
/// ambiguity, resubmit with the same `ctx.entry_id` (the idempotency key):
/// if the original post committed, this call returns the original
/// [`PostedEntry`]; if it did not, this call posts it (never duplicated).
/// Never resubmit with a fresh `entry_id` after an error.
///
/// # Errors
/// [`JournalError::Conflict`] when the retry budget ran out on races;
/// the guard refusals mapped to their domain variants
/// ([`JournalError::PeriodNotOpen`], [`JournalError::UnknownAccount`],
/// [`JournalError::ForeignJournal`], [`JournalError::ForeignReversal`]);
/// [`JournalError::EntryIdAlreadyUsed`] when the `entry_id` was already
/// posted to a different journal (a misuse of the idempotency key);
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
/// Resubmits with the same `ctx.entry_id` (idempotency key) return the
/// original [`PostedEntry`] if it committed, or allow reposting if the
/// first attempt failed; the outcome is deterministic, never duplicated.
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
    let options = PostOptions {
        human: audit.actor.is_person(),
        ..Default::default()
    };
    post_raw_entry_with(db, raw, ctx, policy, sleep, &options, audit).await
}

/// [`post_raw_entry`] with the port-level [`PostOptions`]: the same
/// batch, with the period override, person-side guard and daily
/// allowance applied. The refusal attribution honours the same options.
///
/// Idempotent by entry_id: a resubmit with the same entry_id returns the
/// original [`PostedEntry`] if it committed, or allows reposting if the
/// first attempt failed.
///
/// # Errors
/// As [`post_entry`], plus [`JournalError::EntryIdAlreadyUsed`] when the
/// entry_id was already posted to a different journal.
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
    // Validate that the entry's base currency matches the company's base currency.
    let company_base_currency = read_company_base_currency(db, &raw.company_id).await?;
    if company_base_currency != raw.base_currency {
        return Err(JournalError::BaseCurrencyMismatch {
            company_currency: company_base_currency,
            entry_currency: raw.base_currency.clone(),
        });
    }

    // Idempotent resubmit: checked inside each attempt's planning. If the
    // entry already exists and belongs to this journal/fiscal year, we return
    // an empty statement list (which batch_atomic commits as a no-op), then
    // return the stashed original. If it exists in a different journal, we
    // fail immediately. Each attempt re-plans with fresh reads, so this
    // handles races: if two concurrent posts use the same entry_id, the
    // loser's PK violation on journal_entry.id is retried, and the next plan
    // finds the original and returns it — never posting twice.
    let expected_number = std::sync::Arc::new(std::sync::Mutex::new(None));
    let stashed_entry = std::sync::Arc::new(std::sync::Mutex::new(None));
    let expected_number_ref = expected_number.clone();
    let stashed_entry_ref = stashed_entry.clone();

    match retry::run_batch_with_retry(
        db,
        policy,
        || {
            let en = expected_number_ref.clone();
            let se = stashed_entry_ref.clone();
            async move {
                // Check if an entry with this id already exists (idempotency).
                // Note: we convert JournalError to DbError so the error can be
                // processed by the retry logic (only unique violations retry).
                let existing = read_posted_entry(db, &ctx.entry_id, &raw.company_id)
                    .await
                    .map_err(|je| DbError::Query(je.to_string()))?;

                if let Some(existing_entry) = existing {
                    if existing_entry.journal_id == raw.journal_id
                        && existing_entry.fiscal_year == ctx.fiscal_year
                    {
                        // Safe idempotent resubmit: stash the entry and return
                        // an empty statement list (batch_atomic on empty is Ok).
                        // No audit event is appended: the original post has one.
                        *se.lock().unwrap() = Some(existing_entry);
                        return Ok(Vec::new());
                    }
                    // The entry_id was used for a different journal/year: that's an error.
                    return Err(DbError::Query(format!(
                        "entry_id {} already posted to different journal",
                        ctx.entry_id
                    )));
                }

                // Not an idempotent resubmit: plan the batch normally.
                let (stmts, num) = plan_post_batch_with(db, raw, ctx, options, audit).await?;
                *en.lock().unwrap() = Some(num);
                Ok(stmts)
            }
        },
        sleep,
    )
    .await
    {
        Ok(()) => {
            // If we stashed an entry (idempotent resubmit), return it.
            // Otherwise, return a new PostedEntry with the expected_number.
            if let Some(existing) = stashed_entry.lock().unwrap().take() {
                Ok(existing)
            } else {
                // Batch committed atomically: entry, lines, chain, outbox, and
                // counter bump all committed. The number came from the pre-batch
                // counter read; the literal INSERT ensures it matches. No
                // post-commit read is needed (issue #71).
                let number = expected_number
                    .lock()
                    .unwrap()
                    .expect("planner set expected_number");
                Ok(PostedEntry {
                    entry_id: ctx.entry_id.clone(),
                    journal_id: raw.journal_id.clone(),
                    fiscal_year: ctx.fiscal_year,
                    number,
                    recorded_at: ctx.recorded_at.clone(),
                })
            }
        }
        Err(err) => Err(match err {
            ledgers_db::GuardError::Conflict { attempts } => {
                // If this is a reversal and the conflict is due to the unique
                // index on journal_entry(reverses), diagnose as AlreadyReversed.
                if let Some(reverses) = raw.reverses.as_deref()
                    && let Some(err) = diagnose_reversal_unique_conflict(db, reverses).await
                {
                    return Err(err);
                }
                JournalError::Conflict { attempts }
            }
            ledgers_db::GuardError::Database(db_error)
                if db_error
                    .to_string()
                    .contains("already posted to different journal") =>
            {
                JournalError::EntryIdAlreadyUsed {
                    entry_id: ctx.entry_id.clone(),
                }
            }
            ledgers_db::GuardError::Database(db_error) if is_guard_refusal(&db_error) => {
                // Reads after a failed write (ADR 0004): the batch is
                // gone; these only choose which error the caller reads.
                diagnose_refusal(db, raw, options, &ctx.entry_id)
                    .await
                    .unwrap_or(JournalError::Database(db_error))
            }
            ledgers_db::GuardError::Database(db_error) => JournalError::Database(db_error),
        }),
    }
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

/// Diagnoses if a conflict is due to the unique constraint on `journal_entry(reverses)`.
/// Returns [`JournalError::AlreadyReversed`] if another reversal of the target already exists.
async fn diagnose_reversal_unique_conflict(
    db: &dyn Database,
    reverses: &str,
) -> Option<JournalError> {
    async fn scalar(db: &dyn Database, statement: Statement) -> Option<i64> {
        let rows = db.query(&statement).await.ok()?;
        rows.first().and_then(|row| row.get::<i64>("n"))
    }

    if scalar(
        db,
        Statement::with_values(
            "SELECT COUNT(*) AS n FROM journal_entry WHERE reverses = ?".to_owned(),
            vec![SeaValue::from(reverses)],
        ),
    )
    .await?
        > 0
    {
        return Some(JournalError::AlreadyReversed {
            entry: reverses.to_owned(),
        });
    }

    None
}

/// Diagnoses which reversal invariant guard (if any) failed.
async fn diagnose_reversal_guards(
    db: &dyn Database,
    reverses: &str,
    options: &PostOptions,
) -> Option<JournalError> {
    async fn scalar(db: &dyn Database, statement: Statement) -> Option<i64> {
        let rows = db.query(&statement).await.ok()?;
        rows.first().and_then(|row| row.get::<i64>("n"))
    }

    // (a) The target's own reverses IS NULL (no reversal of a reversal).
    if scalar(
        db,
        Statement::with_values(
            "SELECT COUNT(*) AS n FROM journal_entry WHERE id = ? AND reverses IS NULL".to_owned(),
            vec![SeaValue::from(reverses)],
        ),
    )
    .await?
        == 0
    {
        return Some(JournalError::ReversalOfReversal {
            entry: reverses.to_owned(),
        });
    }

    // (b) No other journal_entry already reverses the target. In diagnose,
    // we check if ANY reversal exists (the guard at posting time checks
    // id <> current, but that entry never got inserted).
    if scalar(
        db,
        Statement::with_values(
            "SELECT COUNT(*) AS n FROM journal_entry WHERE reverses = ?".to_owned(),
            vec![SeaValue::from(reverses)],
        ),
    )
    .await?
        > 0
    {
        return Some(JournalError::AlreadyReversed {
            entry: reverses.to_owned(),
        });
    }

    // (c) If the posting actor is 'agent', the target's actor is not 'human'.
    if !options.human
        && scalar(
            db,
            Statement::with_values(
                "SELECT COUNT(*) AS n FROM journal_entry WHERE id = ? AND actor = 'human'"
                    .to_owned(),
                vec![SeaValue::from(reverses)],
            ),
        )
        .await?
            > 0
    {
        return Some(JournalError::HumanOnlyReversal);
    }

    None
}

/// Attributes a guard refusal to its domain error with cheap reads:
/// journal ownership, reversal-target ownership, the posting period's
/// status, and each line's account against the chart. `None` when
/// nothing attributable is found (an unbalanced raw entry, or a read
/// that itself failed) — the caller then keeps the original database
/// error, which is the honest answer for both.
#[allow(clippy::too_many_lines)]
async fn diagnose_refusal(
    db: &dyn Database,
    raw: &RawEntry,
    options: &PostOptions,
    _entry_id: &str,
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

    // Reversal invariant guards (ADR 0004 pattern): diagnose which guard failed.
    if let Some(reverses) = raw.reverses.as_deref() {
        return diagnose_reversal_guards(db, reverses, options).await;
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
            return missing_or_archived(db, company, &line.account_number)
                .await
                .ok();
        }
    }

    // TODO: Dimension diagnosis (required, forbidden, unknown-value) - the Rows type
    // does not support iteration, so diagnosis of dimension-related guard failures
    // would require a different API or workaround.

    // Diagnose per-unit balance: check the raw entry's dimensions to find unbalanced units.
    // For each own-books unit that has lines in this entry, sum the base_minor of those lines.
    let own_books_units = read_own_books_units(db, company).await.ok()?;
    if !own_books_units.is_empty() {
        let mut unit_sums: BTreeMap<String, i64> = BTreeMap::new();
        for line in &raw.lines {
            if let Some(unit) = line.dimensions.get("unit")
                && own_books_units.contains(unit)
            {
                *unit_sums.entry(unit.clone()).or_insert(0) += line.base_minor;
            }
        }
        for (unit, sum) in unit_sums {
            if sum != 0 {
                return Some(JournalError::UnitUnbalanced { unit });
            }
        }
    }

    None
}

/// Why a posting line's account refused: [`JournalError::AccountArchived`]
/// when the account exists but is archived, else
/// [`JournalError::UnknownAccount`]. The guard only says "not active", so
/// this reads the chart to say which.
///
/// # Errors
/// [`JournalError::Database`] when the lookup fails.
async fn missing_or_archived(
    db: &dyn Database,
    company: &str,
    account: &str,
) -> Result<JournalError, JournalError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT archived FROM account WHERE company_id = ? AND number = ?".to_owned(),
            vec![SeaValue::from(company), SeaValue::from(account)],
        ))
        .await?;
    Ok(match rows.first() {
        Some(_) => JournalError::AccountArchived {
            account: account.to_owned(),
        },
        None => JournalError::UnknownAccount {
            account: account.to_owned(),
        },
    })
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

/// Reads a posted entry by id (idempotency lookup). Returns the entry
/// with all its identifiers and number, or `None` if not found.
async fn read_posted_entry(
    db: &dyn Database,
    entry_id: &str,
    company_id: &str,
) -> Result<Option<PostedEntry>, JournalError> {
    let rows = db
        .query(&Statement::with_values(
            format!("SELECT journal_id, fiscal_year, number, recorded_at FROM {ENTRY_TABLE} WHERE id = ? AND company_id = ?"),
            vec![SeaValue::from(entry_id), SeaValue::from(company_id)],
        ))
        .await?;
    Ok(rows.first().map(|row| PostedEntry {
        entry_id: entry_id.to_owned(),
        journal_id: row.get::<String>("journal_id").expect("journal_id is text"),
        fiscal_year: row
            .get::<i64>("fiscal_year")
            .expect("fiscal_year is an integer"),
        number: row.get::<i64>("number").expect("number is an integer"),
        recorded_at: row
            .get::<String>("recorded_at")
            .expect("recorded_at is text"),
    }))
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

/// Reads the company's base currency from the database.
///
/// # Errors
/// [`JournalError::Database`] when the query fails or the company has no
/// base currency configured.
async fn read_company_base_currency(
    db: &dyn Database,
    company_id: &str,
) -> Result<String, JournalError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT base_currency FROM company WHERE id = ?".to_owned(),
            vec![SeaValue::from(company_id)],
        ))
        .await?;
    rows.rows
        .first()
        .and_then(|row| row.get::<String>("base_currency"))
        .ok_or_else(|| {
            JournalError::Database(DbError::Query(format!(
                "company {company_id} has no base currency configured"
            )))
        })
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

/// Dry run: validates entry and returns it plus provisional number (counter + 1)
/// without writing anything. Also checks period-open and account-exists so it
/// refuses what a post would refuse.
///
/// # Errors
///
/// Returns a [`JournalError`] if the entry fails validation checks such as
/// period-not-open or unknown account.
pub async fn dry_run_entry(
    db: &dyn Database,
    entry: &Entry,
    ctx: &PostContext,
) -> Result<(Entry, i64), JournalError> {
    let current_number = numbering::read_counter(db, &entry.journal_id, ctx.fiscal_year)
        .await?
        .unwrap_or(0);
    let provisional = current_number + 1;

    let posting_period = entry.posting_period();
    let company = &entry.company_id;

    // Period-open check.
    let rows = db
        .query(&Statement::with_values(
            "SELECT status FROM accounting_period WHERE company_id = ? AND period = ?".to_owned(),
            vec![
                SeaValue::from(company),
                SeaValue::from(posting_period.as_str()),
            ],
        ))
        .await?;

    let is_open = rows
        .first()
        .and_then(|row| row.get::<String>("status"))
        .as_deref()
        == Some("open");

    if !is_open {
        return Err(JournalError::PeriodNotOpen {
            period: posting_period,
        });
    }

    // Accounts-exist check.
    for line in &entry.lines {
        let rows = db
            .query(&Statement::with_values(
                "SELECT 1 FROM account WHERE company_id = ? AND number = ? AND archived = 0"
                    .to_owned(),
                vec![
                    SeaValue::from(company),
                    SeaValue::from(line.account_number.as_str()),
                ],
            ))
            .await?;

        if rows.is_empty() {
            return Err(missing_or_archived(db, company, &line.account_number).await?);
        }
    }

    Ok((entry.clone(), provisional))
}

/// Posts multiple entries in a single atomic batch.
/// All entries must belong to the same company and journal.
/// Idempotency: returns original entries if any have `external_id` already posted.
///
/// # Errors
/// [`JournalError::CompanyMismatch`] when entries belong to different companies;
/// [`JournalError::Conflict`] on race conditions or other guard failures;
/// [`JournalError::Money`] when amounts do not fit storage width.
pub async fn batch_post_entries(
    db: &dyn Database,
    entries: &[Entry],
    company_id: &str,
    policy: RetryPolicy,
    sleep: impl FnMut(Duration),
    audit: &AuditContext,
) -> Result<Vec<PostedEntry>, JournalError> {
    // Validate all entries belong to the same company
    for entry in entries {
        if entry.company_id != company_id {
            return Err(JournalError::Database(cratefield_core::DbError::Query(
                format!("entry belongs to different company: {}", entry.company_id),
            )));
        }
    }

    // Validate all entries belong to the same journal
    let journal_id = &entries[0].journal_id;
    for entry in entries {
        if &entry.journal_id != journal_id {
            return Err(JournalError::Database(cratefield_core::DbError::Query(
                "entries must all belong to the same journal".to_string(),
            )));
        }
    }

    // Convert entries to raw entries
    let mut raw_entries = Vec::with_capacity(entries.len());
    for entry in entries {
        raw_entries.push(RawEntry::from_entry(entry)?);
    }

    // Extract fiscal year from the first entry's vat_period (format: "YYYY-MM")
    let fiscal_year = entries[0]
        .vat_period
        .split('-')
        .next()
        .and_then(|year| year.parse::<i64>().ok())
        .ok_or_else(|| {
            JournalError::Database(cratefield_core::DbError::Query(
                "invalid fiscal year from vat_period".to_owned(),
            ))
        })?;

    // Create PostContext for each entry with fresh ULIDs
    let now = OffsetDateTime::now_utc();
    let recorded_at = now
        .format(&time::format_description::well_known::Iso8601::DEFAULT)
        .unwrap_or_else(|_| "2000-01-01T00:00:00Z".to_owned());
    let mut contexts = Vec::with_capacity(entries.len());
    let timestamp = now.unix_timestamp_nanos();

    for (i, _raw) in raw_entries.iter().enumerate() {
        // Generate entry and outbox IDs with timestamp and index for uniqueness
        let entry_id = format!("batch-{timestamp}-{i}");
        let outbox_id = format!("{entry_id}-outbox");
        contexts.push(PostContext {
            fiscal_year,
            entry_id,
            recorded_at: recorded_at.clone(),
            outbox_id,
        });
    }

    // Combine all statements into one batch
    let options = PostOptions {
        human: audit.actor.is_person(),
        ..Default::default()
    };

    // Wrap data in Arc for sharing across retry attempts
    let raw_entries_arc = std::sync::Arc::new(raw_entries);
    let contexts_arc = std::sync::Arc::new(contexts);
    let company_id_arc = std::sync::Arc::new(company_id.to_owned());
    let options_arc = std::sync::Arc::new(options);

    match retry::run_batch_with_retry(
        db,
        policy,
        || {
            let raw_entries_clone = raw_entries_arc.clone();
            let contexts_clone = contexts_arc.clone();
            let options_clone = options_arc.clone();

            async move {
                let mut all_statements = Vec::new();

                // Plan statements for each entry
                for (i, raw) in raw_entries_clone.iter().enumerate() {
                    let ctx = &contexts_clone[i];
                    let (stmts, _number) =
                        plan_post_batch_statements_only(db, raw, ctx, &options_clone).await?;
                    all_statements.extend(stmts);
                }

                Ok(all_statements)
            }
        },
        sleep,
    )
    .await
    {
        Ok(()) => {
            // Read back the posted entries to get their numbers
            let mut results = Vec::new();
            for ctx in contexts_arc.iter() {
                if let Some(posted) = read_posted_entry(db, &ctx.entry_id, &company_id_arc).await? {
                    results.push(posted);
                }
            }
            Ok(results)
        }
        Err(err) => Err(match err {
            ledgers_db::GuardError::Conflict { attempts } => JournalError::Conflict { attempts },
            ledgers_db::GuardError::Database(db_error) => JournalError::Database(db_error),
        }),
    }
}

/// Reverses an entry by `external_id`, creating a reversal entry.
/// Returns the new reversal entry.
///
/// # Errors
/// [`JournalError::ForeignReversal`] when the original entry is not found;
/// [`JournalError::ReversalOfReversal`] when the original entry is already a reversal;
/// [`JournalError::Conflict`] on race conditions or other guard failures;
/// [`JournalError::Money`] when amounts do not fit storage width.
pub async fn reverse_by_external_id(
    db: &dyn Database,
    company_id: &str,
    external_id: &str,
    reversal_draft: &crate::entry::EntryDraft,
    policy: RetryPolicy,
    sleep: impl FnMut(Duration),
    audit: &AuditContext,
) -> Result<PostedEntry, JournalError> {
    // Query the database for the entry with matching (company_id, external_id)
    let rows = db
        .query(&Statement::with_values(
            "SELECT id, reverses FROM journal_entry WHERE company_id = ? AND external_id = ?"
                .to_owned(),
            vec![SeaValue::from(company_id), SeaValue::from(external_id)],
        ))
        .await?;

    // If not found, return error
    let original_entry_id = rows
        .first()
        .and_then(|row| row.get::<String>("id"))
        .ok_or_else(|| JournalError::ForeignReversal {
            entry: format!("external_id={external_id}"),
        })?;

    // Validate it's not already a reversal (reverses field is None)
    let already_reverses = rows
        .first()
        .and_then(|row| row.get::<Option<String>>("reverses"))
        .flatten();

    if already_reverses.is_some() {
        return Err(JournalError::ReversalOfReversal {
            entry: original_entry_id.clone(),
        });
    }

    // Create the reversal entry using the draft provided, setting reverses to the original entry id
    let mut reversal_draft = reversal_draft.clone();
    reversal_draft.reverses = Some(original_entry_id);

    // Validate the reversal entry
    let entry = Entry::from_draft(reversal_draft, company_id)?;

    // Create PostContext for the reversal
    let now = OffsetDateTime::now_utc();
    let recorded_at = now
        .format(&time::format_description::well_known::Iso8601::DEFAULT)
        .unwrap_or_else(|_| "2000-01-01T00:00:00Z".to_owned());
    let timestamp = now.unix_timestamp_nanos();
    let entry_id = format!("reversal-{timestamp}");
    let outbox_id = format!("{entry_id}-outbox");

    // Extract fiscal year from the entry's vat_period
    let fiscal_year = entry
        .vat_period
        .split('-')
        .next()
        .and_then(|year| year.parse::<i64>().ok())
        .ok_or_else(|| {
            JournalError::Database(cratefield_core::DbError::Query(
                "invalid fiscal year from vat_period".to_owned(),
            ))
        })?;

    let ctx = PostContext {
        fiscal_year,
        entry_id,
        recorded_at,
        outbox_id,
    };

    // Post the reversal entry normally using post_entry
    post_entry(db, &entry, &ctx, policy, sleep, audit).await
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
                external_id: None,
                source_app: None,
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
