//! Posting and settling a period on both sides, or on neither.
//!
//! Each side's entry goes through that company's own executor, which is the
//! server's job: the [`SideExecutor`] port is where it runs. The order is fixed:
//! dry-run both entries, post the licensor's, then the licensee's. If the
//! licensee's post fails, the licensor's entry is reversed by its external id
//! and both documents are marked failed. Settlement follows the same rule
//! without the failed mark: a failed settlement leaves the period posted.
//!
//! The document rows are updated in one atomic batch after the entries exist.
//! A crash between the posts and that batch leaves the documents in draft with
//! their attempt number advanced; the next call finds the prior attempt's
//! entries and adopts them instead of posting again.

use crate::accrual::{self, EntryContext, Step};
use crate::model::{DocStatus, PeriodAmounts, PeriodDocView, Side};
use crate::store::{self, AgreementRow, DocRow};
use async_trait::async_trait;
use cratefield_core::{Database, Statement};
use ledgers_commands::{Command, CommandContext, CommandError};
use ledgers_ledger::surface::{PostEntry, PostEntryInput};
use sea_query::Value;

/// The port a side's entry is posted through: the company's own executor.
#[async_trait]
pub trait SideExecutor: Send + Sync {
    /// Posts `input` into `company_id` and returns the new entry id.
    ///
    /// # Errors
    /// The message the company's executor refused the entry with.
    async fn post(&self, company_id: &str, input: &PostEntryInput) -> Result<String, String>;

    /// Reverses the entry posted into `company_id` with `external_id`.
    ///
    /// # Errors
    /// The message the company's executor refused the reversal with.
    async fn reverse(
        &self,
        company_id: &str,
        external_id: &str,
        posting_date: &str,
    ) -> Result<(), String>;
}

/// The result of a post or settle call: the two documents as they stand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeriodResult {
    /// The agreement.
    pub agreement_id: String,
    /// The period, `YYYY-MM`.
    pub period: String,
    /// The documents, licensor first.
    pub docs: Vec<PeriodDocView>,
}

/// The intercompany id both sides' documents share.
#[must_use]
pub fn intercompany_id(agreement_id: &str, period: &str) -> String {
    format!("IC-{agreement_id}-{period}")
}

/// The two rows of a pair, licensor first.
fn pair_of(rows: Vec<AgreementRow>) -> Result<(AgreementRow, AgreementRow), CommandError> {
    let mut licensor = None;
    let mut licensee = None;
    for row in rows {
        match row.view.side {
            Side::Licensor => licensor = Some(row),
            Side::Licensee => licensee = Some(row),
        }
    }
    match (licensor, licensee) {
        (Some(licensor), Some(licensee)) => Ok((licensor, licensee)),
        _ => Err(CommandError::InvalidInput(
            "the agreement does not have both sides".into(),
        )),
    }
}

/// The two documents of a period, licensor first.
fn docs_pair(rows: Vec<DocRow>, id: &str) -> Result<(DocRow, DocRow), CommandError> {
    let mut licensor = None;
    let mut licensee = None;
    for row in rows {
        match row.view.side {
            Side::Licensor => licensor = Some(row),
            Side::Licensee => licensee = Some(row),
        }
    }
    licensor.zip(licensee).ok_or_else(|| {
        CommandError::InvalidInput(format!("{id} has no drafts: run the period first"))
    })
}

fn amounts_of(doc: &PeriodDocView) -> PeriodAmounts {
    PeriodAmounts {
        basis_minor: doc.basis_minor,
        gross_minor: doc.gross_minor,
        wht_minor: doc.wht_minor,
        net_minor: doc.net_minor,
    }
}

fn period_result(
    agreement_id: &str,
    period: &str,
    licensor: DocRow,
    licensee: DocRow,
) -> PeriodResult {
    PeriodResult {
        agreement_id: agreement_id.to_owned(),
        period: period.to_owned(),
        docs: vec![licensor.view, licensee.view],
    }
}

/// The general journal of a company, by its code `GN`.
async fn general_journal(db: &dyn Database, company_id: &str) -> Result<String, CommandError> {
    ledgers_ledger::journal::find_journal(db, company_id, "GN")
        .await
        .map_err(|err| CommandError::InvalidInput(err.to_string()))?
        .map(|journal| journal.id)
        .ok_or_else(|| {
            CommandError::InvalidInput(format!("no general journal GN exists for {company_id}"))
        })
}

fn fiscal_year(period: &str) -> Result<i64, CommandError> {
    period
        .get(..4)
        .and_then(|year| year.parse().ok())
        .ok_or_else(|| CommandError::InvalidInput(format!("{period:?} is not a YYYY-MM period")))
}

/// The command context a company's validation runs in.
fn context(company_id: &str, request: &str, now: &str) -> CommandContext {
    CommandContext {
        company_id: company_id.to_owned(),
        request_id: request.to_owned(),
        event_id: format!("{request}-{company_id}"),
        now: now.to_owned(),
        is_person: true, // Internal orchestration, treat as person
    }
}

/// A dry run of one side's entry: the ledger's own validation, with no write.
async fn dry_run(
    db: &dyn Database,
    company_id: &str,
    request: &str,
    now: &str,
    input: &PostEntryInput,
) -> Result<(), CommandError> {
    let ctx = context(company_id, request, now);
    PostEntry.preview(db, &ctx, input).await.map(|_| ())
}

fn update_docs(
    id: &str,
    status: DocStatus,
    entry_ids: [Option<&str>; 2],
    attempt: Option<i64>,
    error: Option<&str>,
    now: &str,
    expected: &[DocStatus],
) -> Vec<Statement> {
    // One statement per side: each row keeps its own entry id.
    let mut out = Vec::new();
    for (side_index, entry) in entry_ids.iter().enumerate() {
        let side = if side_index == 0 {
            "licensor"
        } else {
            "licensee"
        };
        let stamp_entry = match status {
            DocStatus::Posted => "accrual_entry_id",
            DocStatus::Settled => "settle_entry_id",
            _ => "",
        };
        let mut sets = vec![
            "status = ?".to_owned(),
            "last_error = ?".to_owned(),
            "updated_at = ?".to_owned(),
        ];
        let mut values = vec![
            Value::from(status.as_str()),
            error.map_or(Value::String(None), Value::from),
            Value::from(now),
        ];
        if !stamp_entry.is_empty() {
            sets.push(format!("{stamp_entry} = ?"));
            values.push(entry.map_or(Value::String(None), Value::from));
        }
        if let Some(attempt) = attempt {
            sets.push("attempt = ?".to_owned());
            values.push(Value::from(attempt));
        }
        let statuses = expected
            .iter()
            .map(|s| format!("'{}'", s.as_str()))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "UPDATE intercompany_document SET {} WHERE id = ? AND side = ? AND status IN ({statuses})",
            sets.join(", ")
        );
        values.push(Value::from(id));
        values.push(Value::from(side));
        out.push(Statement::with_values(sql, values));
    }
    out
}

/// Marks the attempt on both documents before any entry is posted, so a retry
/// after a crash never reuses the external id of an entry that exists.
fn start_attempt(id: &str, attempt: i64, now: &str) -> Vec<Statement> {
    Side::BOTH
        .into_iter()
        .map(|side| {
            Statement::with_values(
                "UPDATE intercompany_document SET attempt = ?, last_error = NULL, updated_at = ? \
                 WHERE id = ? AND side = ? AND status IN ('draft', 'failed')"
                    .to_owned(),
                vec![
                    Value::from(attempt),
                    Value::from(now),
                    Value::from(id),
                    Value::from(side.as_str()),
                ],
            )
        })
        .collect()
}

/// Posts one period on both sides, or on neither.
///
/// # Errors
/// [`CommandError::InvalidInput`] when the period has no drafts, a side's
/// entry fails its dry run or its post, or the agreement is not in a state to
/// post; [`CommandError::Database`] when a read or the status batch fails.
#[allow(clippy::too_many_lines)]
pub async fn post_period(
    db: &dyn Database,
    sides: &dyn SideExecutor,
    agreement_id: &str,
    period: &str,
    call_id: &str,
    now: &str,
) -> Result<PeriodResult, CommandError> {
    let (licensor_row, licensee_row) = pair_of(store::agreement_sides(db, agreement_id).await?)?;
    let id = intercompany_id(agreement_id, period);
    let (licensor_doc, licensee_doc) =
        docs_pair(store::docs_for_period(db, agreement_id, period).await?, &id)?;

    // Already posted, settled or both at once: nothing to do.
    let both = (licensor_doc.view.status, licensee_doc.view.status);
    match both {
        (DocStatus::Posted | DocStatus::Settled, DocStatus::Posted | DocStatus::Settled) => {
            return Ok(period_result(
                agreement_id,
                period,
                licensor_doc,
                licensee_doc,
            ));
        }
        (DocStatus::Draft | DocStatus::Failed, DocStatus::Draft | DocStatus::Failed) => {}
        _ => {
            return Err(CommandError::InvalidInput(format!(
                "{id} is {} for the licensor and {} for the licensee; it cannot post",
                licensor_doc.view.status.as_str(),
                licensee_doc.view.status.as_str()
            )));
        }
    }

    // A zero period produces nothing to post: it is settled at once.
    if licensor_doc.view.gross_minor == 0 && licensee_doc.view.gross_minor == 0 {
        let statements = update_docs(
            &id,
            DocStatus::Settled,
            [None, None],
            None,
            None,
            now,
            &[DocStatus::Draft, DocStatus::Failed],
        );
        db.batch_atomic(&statements).await?;
        let (licensor_doc, licensee_doc) = reload(db, agreement_id, period, &id).await?;
        return Ok(period_result(
            agreement_id,
            period,
            licensor_doc,
            licensee_doc,
        ));
    }

    let attempt = licensor_doc.attempt.max(licensee_doc.attempt) + 1;
    let suffix = attempt.to_string();
    let journal_licensor = general_journal(db, &licensor_row.company_id).await?;
    let journal_licensee = general_journal(db, &licensee_row.company_id).await?;
    let year = fiscal_year(period)?;
    let period_end = crate::model::period_end(period)
        .ok_or_else(|| CommandError::InvalidInput(format!("{period:?} is not a YYYY-MM period")))?;

    let licensor_input = accrual::entry_for(
        Side::Licensor,
        Step::Accrue,
        &licensor_row.view.accounts,
        &amounts_of(&licensor_doc.view),
        &EntryContext {
            journal_id: &journal_licensor,
            fiscal_year: year,
            period_end: &period_end,
            period,
            agreement_id,
            currency: &licensor_doc.view.currency,
            suffix: &suffix,
        },
    )?;
    let licensee_input = accrual::entry_for(
        Side::Licensee,
        Step::Accrue,
        &licensee_row.view.accounts,
        &amounts_of(&licensee_doc.view),
        &EntryContext {
            journal_id: &journal_licensee,
            fiscal_year: year,
            period_end: &period_end,
            period,
            agreement_id,
            currency: &licensee_doc.view.currency,
            suffix: &suffix,
        },
    )?;

    // Dry-run both sides before anything is written.
    dry_run(db, &licensor_row.company_id, call_id, now, &licensor_input).await?;
    dry_run(db, &licensee_row.company_id, call_id, now, &licensee_input).await?;

    // Adopt the entries of a prior attempt that posted but whose status batch
    // did not land: the entries exist, so posting them again would double them.
    if let Some((licensor_entry, licensee_entry)) = adopt_prior(
        db,
        &licensor_row,
        &licensee_row,
        agreement_id,
        period,
        &licensor_doc,
        &licensee_doc,
    )
    .await?
    {
        let statements = posted_statements(&id, &licensor_entry, &licensee_entry, now);
        db.batch_atomic(&statements).await?;
        let (licensor_doc, licensee_doc) = reload(db, agreement_id, period, &id).await?;
        return Ok(period_result(
            agreement_id,
            period,
            licensor_doc,
            licensee_doc,
        ));
    }

    db.batch_atomic(&start_attempt(&id, attempt, now)).await?;

    let licensor_ext =
        accrual::external_id(agreement_id, period, Side::Licensor, Step::Accrue, &suffix);
    let licensor_entry = match sides.post(&licensor_row.company_id, &licensor_input).await {
        Ok(entry) => entry,
        Err(message) => {
            let message = format!("the licensor side did not post: {message}");
            fail_both(db, &id, &message, now).await?;
            return Err(CommandError::InvalidInput(message));
        }
    };

    let licensee_entry = match sides.post(&licensee_row.company_id, &licensee_input).await {
        Ok(entry) => entry,
        Err(message) => {
            let reversal = sides
                .reverse(&licensor_row.company_id, &licensor_ext, &period_end)
                .await;
            let message = match reversal {
                Ok(()) => format!(
                    "the licensee side did not post, so the licensor side was reversed: {message}"
                ),
                Err(reason) => format!(
                    "the licensee side did not post: {message}; reversing the licensor side also failed: {reason}"
                ),
            };
            fail_both(db, &id, &message, now).await?;
            return Err(CommandError::InvalidInput(message));
        }
    };

    let statements = posted_statements(&id, &licensor_entry, &licensee_entry, now);
    db.batch_atomic(&statements).await?;
    let (licensor_doc, licensee_doc) = reload(db, agreement_id, period, &id).await?;
    Ok(period_result(
        agreement_id,
        period,
        licensor_doc,
        licensee_doc,
    ))
}

/// The entries of a prior attempt that are still in the books, when both
/// sides have one and neither was reversed.
async fn adopt_prior(
    db: &dyn Database,
    licensor: &AgreementRow,
    licensee: &AgreementRow,
    agreement_id: &str,
    period: &str,
    licensor_doc: &DocRow,
    licensee_doc: &DocRow,
) -> Result<Option<(String, String)>, CommandError> {
    if licensor_doc.attempt == 0 || licensor_doc.view.status != DocStatus::Draft {
        return Ok(None);
    }
    let prior = (licensor_doc.attempt).max(licensee_doc.attempt).to_string();
    let l = ledgers_ledger::query::entry_by_external_id(
        db,
        &licensor.company_id,
        &accrual::external_id(agreement_id, period, Side::Licensor, Step::Accrue, &prior),
    )
    .await
    .map_err(|err| CommandError::InvalidInput(err.to_string()))?;
    let e = ledgers_ledger::query::entry_by_external_id(
        db,
        &licensee.company_id,
        &accrual::external_id(agreement_id, period, Side::Licensee, Step::Accrue, &prior),
    )
    .await
    .map_err(|err| CommandError::InvalidInput(err.to_string()))?;
    Ok(match (l, e) {
        (Some(l), Some(e)) if l.reversed_by.is_empty() && e.reversed_by.is_empty() => {
            Some((l.id, e.id))
        }
        _ => None,
    })
}

fn posted_statements(
    id: &str,
    licensor_entry: &str,
    licensee_entry: &str,
    now: &str,
) -> Vec<Statement> {
    update_docs(
        id,
        DocStatus::Posted,
        [Some(licensor_entry), Some(licensee_entry)],
        None,
        None,
        now,
        &[DocStatus::Draft, DocStatus::Failed],
    )
}

async fn fail_both(
    db: &dyn Database,
    id: &str,
    message: &str,
    now: &str,
) -> Result<(), CommandError> {
    let statements = update_docs(
        id,
        DocStatus::Failed,
        [None, None],
        None,
        Some(message),
        now,
        &[DocStatus::Draft, DocStatus::Failed],
    );
    db.batch_atomic(&statements).await?;
    Ok(())
}

async fn reload(
    db: &dyn Database,
    agreement_id: &str,
    period: &str,
    id: &str,
) -> Result<(DocRow, DocRow), CommandError> {
    docs_pair(store::docs_for_period(db, agreement_id, period).await?, id)
}

/// Settles a posted period on both sides, or on neither. The payment is the
/// licensee's net to the bank and the licensor's receipt of the same net, with
/// the withholding debited to the licensor's withholding receivable.
///
/// # Errors
/// [`CommandError::InvalidInput`] when the period is not posted, a side's
/// entry fails its dry run or its post; [`CommandError::Database`] on a
/// failed read or batch.
#[allow(clippy::too_many_lines)]
pub async fn settle_period(
    db: &dyn Database,
    sides: &dyn SideExecutor,
    agreement_id: &str,
    period: &str,
    call_id: &str,
    now: &str,
) -> Result<PeriodResult, CommandError> {
    let (licensor_row, licensee_row) = pair_of(store::agreement_sides(db, agreement_id).await?)?;
    let id = intercompany_id(agreement_id, period);
    let (licensor_doc, licensee_doc) =
        docs_pair(store::docs_for_period(db, agreement_id, period).await?, &id)?;
    match (licensor_doc.view.status, licensee_doc.view.status) {
        (DocStatus::Settled, DocStatus::Settled) => {
            return Ok(period_result(
                agreement_id,
                period,
                licensor_doc,
                licensee_doc,
            ));
        }
        (DocStatus::Posted, DocStatus::Posted) => {}
        _ => {
            return Err(CommandError::InvalidInput(format!(
                "{id} must be posted before it settles (licensor {}, licensee {})",
                licensor_doc.view.status.as_str(),
                licensee_doc.view.status.as_str()
            )));
        }
    }

    let journal_licensor = general_journal(db, &licensor_row.company_id).await?;
    let journal_licensee = general_journal(db, &licensee_row.company_id).await?;
    let year = fiscal_year(period)?;
    let period_end = crate::model::period_end(period)
        .ok_or_else(|| CommandError::InvalidInput(format!("{period:?} is not a YYYY-MM period")))?;
    // The settlement's external ids carry the posting attempt, which is fixed
    // once the period has posted, so a retry names the same entries. A
    // per-call id here let a retry post a settled side a second time.
    let suffix = licensor_doc.attempt.max(licensee_doc.attempt).to_string();
    let licensor_input = accrual::entry_for(
        Side::Licensor,
        Step::Settle,
        &licensor_row.view.accounts,
        &amounts_of(&licensor_doc.view),
        &EntryContext {
            journal_id: &journal_licensor,
            fiscal_year: year,
            period_end: &period_end,
            period,
            agreement_id,
            currency: &licensor_doc.view.currency,
            suffix: &suffix,
        },
    )?;
    let licensee_input = accrual::entry_for(
        Side::Licensee,
        Step::Settle,
        &licensee_row.view.accounts,
        &amounts_of(&licensee_doc.view),
        &EntryContext {
            journal_id: &journal_licensee,
            fiscal_year: year,
            period_end: &period_end,
            period,
            agreement_id,
            currency: &licensee_doc.view.currency,
            suffix: &suffix,
        },
    )?;

    // A side an earlier call already posted is adopted, not posted again.
    let licensor_ext =
        accrual::external_id(agreement_id, period, Side::Licensor, Step::Settle, &suffix);
    let licensee_ext =
        accrual::external_id(agreement_id, period, Side::Licensee, Step::Settle, &suffix);
    let licensor_prior = live_entry(db, &licensor_row.company_id, &licensor_ext).await?;
    let licensee_prior = live_entry(db, &licensee_row.company_id, &licensee_ext).await?;
    if licensor_prior.is_none() {
        dry_run(db, &licensor_row.company_id, call_id, now, &licensor_input).await?;
    }
    if licensee_prior.is_none() {
        dry_run(db, &licensee_row.company_id, call_id, now, &licensee_input).await?;
    }

    // A failing side does not reverse the other. The entry that landed stays
    // and the period stays posted; the next call adopts it and posts the rest.
    let licensor_entry = match licensor_prior {
        Some(entry) => entry,
        None => sides
            .post(&licensor_row.company_id, &licensor_input)
            .await
            .map_err(|message| {
                CommandError::InvalidInput(format!("the licensor side did not settle: {message}"))
            })?,
    };
    let licensee_entry = match licensee_prior {
        Some(entry) => entry,
        None => sides
            .post(&licensee_row.company_id, &licensee_input)
            .await
            .map_err(|message| {
                CommandError::InvalidInput(format!(
                    "the licensee side did not settle: {message}; the licensor side stays settled and the next call settles the licensee side"
                ))
            })?,
    };

    let statements = update_docs(
        &id,
        DocStatus::Settled,
        [Some(&licensor_entry), Some(&licensee_entry)],
        None,
        None,
        now,
        &[DocStatus::Posted],
    );
    db.batch_atomic(&statements).await?;
    let (licensor_doc, licensee_doc) = reload(db, agreement_id, period, &id).await?;
    Ok(period_result(
        agreement_id,
        period,
        licensor_doc,
        licensee_doc,
    ))
}

/// The id of the entry with `external_id` on `company_id` when it is in the
/// books. An entry that was reversed cannot be adopted: the settlement would
/// have to start under a new id, which only an operator should decide.
async fn live_entry(
    db: &dyn Database,
    company_id: &str,
    external_id: &str,
) -> Result<Option<String>, CommandError> {
    let Some(entry) = ledgers_ledger::query::entry_by_external_id(db, company_id, external_id)
        .await
        .map_err(|err| CommandError::InvalidInput(err.to_string()))?
    else {
        return Ok(None);
    };
    if entry.reversed_by.is_empty() {
        Ok(Some(entry.id))
    } else {
        Err(CommandError::InvalidInput(format!(
            "settlement entry {external_id} was reversed; the period cannot settle under it"
        )))
    }
}
