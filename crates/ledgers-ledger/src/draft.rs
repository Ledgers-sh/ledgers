//! Entry drafts: saving, editing, listing, and posting unposted entries.
//! A draft is "posted" iff a `journal_entry` with matching id exists, so the
//! post operation is just `post_entry` with the draft's id as `ctx.entry_id`.

use crate::entry::{Entry, EntryDraft, Fx, JournalError, Line, Side};
use crate::posting::{PostContext, PostedEntry, post_entry};
use cratefield_core::{Database, Statement};
use ledgers_audit::AuditContext;
use ledgers_db::guard;
use ledgers_db::retry::RetryPolicy;
use ledgers_money::{Amount, Currency};
use sea_query::Value as SeaValue;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

/// Serializable representation of `EntryDraft` for storage in JSON.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct StoredEntryDraft {
    pub journal_id: String,
    pub document_date: String, // ISO "YYYY-MM-DD"
    pub posting_date: String,
    pub tax_point_date: Option<String>,
    pub base_currency: String, // Currency code
    pub description: String,
    pub source_documents: Vec<String>,
    pub reverses: Option<String>,
    pub lines: Vec<StoredLine>,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct StoredLine {
    pub account_number: String,
    pub side: String,    // "debit" or "credit"
    pub base_minor: i64, // Minor units
    pub base_currency: String,
    pub fx: Option<StoredFx>,
    pub tax_code: Option<String>,
    pub dimensions: BTreeMap<String, String>,
    pub memo: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct StoredFx {
    pub transaction_minor: i64,
    pub transaction_currency: String,
    pub rate_scaled: String, // Rate × 10^12 as string
}

impl StoredEntryDraft {
    /// Convert a domain `EntryDraft` to storable form.
    #[must_use]
    pub fn from_draft(draft: &EntryDraft) -> Self {
        let format_date =
            |d: time::Date| format!("{:04}-{:02}-{:02}", d.year(), u8::from(d.month()), d.day());

        StoredEntryDraft {
            journal_id: draft.journal_id.clone(),
            document_date: format_date(draft.document_date),
            posting_date: format_date(draft.posting_date),
            tax_point_date: draft.tax_point_date.map(format_date),
            base_currency: draft.base_currency.code().to_owned(),
            description: draft.description.clone(),
            source_documents: draft.source_documents.clone(),
            reverses: draft.reverses.clone(),
            lines: draft
                .lines
                .iter()
                .map(|line| StoredLine {
                    account_number: line.account_number.clone(),
                    side: line.side.to_string(),
                    base_minor: line.side.sign() * i64::try_from(line.base.minor()).unwrap_or(0),
                    base_currency: draft.base_currency.code().to_owned(),
                    fx: line.fx.map(|fx| StoredFx {
                        transaction_minor: line.side.sign()
                            * i64::try_from(fx.transaction.minor()).unwrap_or(0),
                        transaction_currency: fx.transaction.currency().code().to_owned(),
                        rate_scaled: fx.rate.scaled().to_string(),
                    }),
                    tax_code: line.tax_code.clone(),
                    dimensions: line.dimensions.clone(),
                    memo: line.memo.clone(),
                })
                .collect(),
        }
    }

    /// Convert from stored form back to domain `EntryDraft`.
    ///
    /// # Errors
    /// Returns `JournalError` on parse failures or invalid data.
    pub fn into_draft(self) -> Result<EntryDraft, JournalError> {
        let parse_date = |s: &str| {
            time::Date::parse(s, &time::format_description::well_known::Iso8601::DEFAULT).map_err(
                |_| {
                    JournalError::Database(cratefield_core::DbError::Query(format!(
                        "invalid date: {s}"
                    )))
                },
            )
        };

        let base_currency = Currency::from_code(&self.base_currency).map_err(|_| {
            JournalError::Database(cratefield_core::DbError::Query(format!(
                "unknown currency: {}",
                self.base_currency
            )))
        })?;

        let mut lines = Vec::new();
        for stored_line in self.lines {
            let side = match stored_line.side.as_str() {
                "debit" => Side::Debit,
                "credit" => Side::Credit,
                _ => {
                    return Err(JournalError::Database(cratefield_core::DbError::Query(
                        format!("unknown side: {}", stored_line.side),
                    )));
                }
            };

            let base_amount = Amount::new(i128::from(stored_line.base_minor.abs()), base_currency);

            let fx = if let Some(stored_fx) = stored_line.fx {
                let tx_currency =
                    Currency::from_code(&stored_fx.transaction_currency).map_err(|_| {
                        JournalError::Database(cratefield_core::DbError::Query(format!(
                            "unknown currency: {}",
                            stored_fx.transaction_currency
                        )))
                    })?;
                let rate = ledgers_money::Rate::parse(&stored_fx.rate_scaled).map_err(|_| {
                    JournalError::Database(cratefield_core::DbError::Query(format!(
                        "invalid rate: {}",
                        stored_fx.rate_scaled
                    )))
                })?;
                let tx_amount =
                    Amount::new(i128::from(stored_fx.transaction_minor.abs()), tx_currency);
                Some(Fx {
                    transaction: tx_amount,
                    rate,
                })
            } else {
                None
            };

            lines.push(Line {
                account_number: stored_line.account_number,
                side,
                base: base_amount,
                fx,
                tax_code: stored_line.tax_code,
                dimensions: stored_line.dimensions,
                memo: stored_line.memo,
            });
        }

        Ok(EntryDraft {
            journal_id: self.journal_id,
            document_date: parse_date(&self.document_date)?,
            posting_date: parse_date(&self.posting_date)?,
            tax_point_date: if let Some(s) = self.tax_point_date {
                Some(parse_date(&s)?)
            } else {
                None
            },
            base_currency,
            description: self.description,
            source_documents: self.source_documents,
            reverses: self.reverses,
            lines,
        })
    }
}

/// Saves an unposted entry draft: inserts a new row in `entry_draft`.
/// If a draft with this id already exists, the operation is idempotent
/// (the row exists with the new body).
///
/// # Errors
/// [`JournalError::Database`] on database errors.
pub async fn save_draft(
    db: &dyn Database,
    company_id: &str,
    draft_id: &str,
    draft: &EntryDraft,
    recorded_at: &str,
) -> Result<(), JournalError> {
    let stored = StoredEntryDraft::from_draft(draft);
    let body = serde_json::to_string(&stored)
        .map_err(|e| JournalError::Database(cratefield_core::DbError::Query(e.to_string())))?;

    db.execute(&Statement::with_values(
        "INSERT INTO entry_draft (id, company_id, journal_id, body, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET body = excluded.body, updated_at = excluded.updated_at"
            .to_owned(),
        vec![
            SeaValue::from(draft_id),
            SeaValue::from(company_id),
            SeaValue::from(draft.journal_id.as_str()),
            SeaValue::from(body.as_str()),
            SeaValue::from(recorded_at),
            SeaValue::from(recorded_at),
        ],
    ))
    .await?;

    Ok(())
}

/// Updates an unposted draft: fails if the draft has already been posted
/// (a `journal_entry` with matching id exists). The guard is a write-time
/// check inside a batch, not a prior read.
///
/// # Errors
/// [`JournalError::EntryIdAlreadyUsed`] if the draft was already posted.
/// [`JournalError::Database`] on other database errors.
pub async fn edit_draft(
    db: &dyn Database,
    company_id: &str,
    draft_id: &str,
    draft: &EntryDraft,
    recorded_at: &str,
) -> Result<(), JournalError> {
    let stored = StoredEntryDraft::from_draft(draft);
    let body = serde_json::to_string(&stored)
        .map_err(|e| JournalError::Database(cratefield_core::DbError::Query(e.to_string())))?;

    // Guard: the draft must not be posted (no journal_entry with this id).
    let guard_key = guard::guard_key(&["draft-not-posted", draft_id]);
    let statements = vec![
        guard::assert_statement(
            &guard_key,
            "(SELECT COUNT(*) FROM journal_entry WHERE id = ?) = 0",
            vec![SeaValue::from(draft_id)],
        ),
        Statement::with_values(
            "UPDATE entry_draft SET body = ?, updated_at = ? WHERE id = ? AND company_id = ?"
                .to_owned(),
            vec![
                SeaValue::from(body.as_str()),
                SeaValue::from(recorded_at),
                SeaValue::from(draft_id),
                SeaValue::from(company_id),
            ],
        ),
        guard::release_statement(&guard_key),
    ];

    db.batch_atomic(&statements).await?;
    Ok(())
}

/// Lists unposted drafts for a company: returns all `entry_draft` rows where
/// no corresponding `journal_entry` exists.
///
/// # Errors
/// [`JournalError::Database`] on database errors.
pub async fn list_drafts(
    db: &dyn Database,
    company_id: &str,
) -> Result<Vec<(String, EntryDraft)>, JournalError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT id, body FROM entry_draft WHERE company_id = ? AND id NOT IN \
             (SELECT id FROM journal_entry) ORDER BY created_at DESC"
                .to_owned(),
            vec![SeaValue::from(company_id)],
        ))
        .await?;

    let drafts = rows
        .rows
        .iter()
        .filter_map(|row| {
            let id = row.get::<String>("id")?;
            let body_json = row.get::<String>("body")?;
            let stored = serde_json::from_str::<StoredEntryDraft>(&body_json).ok()?;
            let draft = stored.into_draft().ok()?;
            Some((id, draft))
        })
        .collect();

    Ok(drafts)
}

/// Shows one unposted draft by id: returns the `entry_draft` row if found and
/// not yet posted.
///
/// # Errors
/// [`JournalError::ForeignReversal`] if the draft doesn't exist (reusing the
/// error for "entry not found" to keep the error set small).
/// [`JournalError::Database`] on database errors.
pub async fn show_draft(
    db: &dyn Database,
    company_id: &str,
    draft_id: &str,
) -> Result<EntryDraft, JournalError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT body FROM entry_draft WHERE id = ? AND company_id = ? AND id NOT IN \
             (SELECT id FROM journal_entry)"
                .to_owned(),
            vec![SeaValue::from(draft_id), SeaValue::from(company_id)],
        ))
        .await?;

    let row = rows.first().ok_or(JournalError::ForeignReversal {
        entry: draft_id.to_owned(),
    })?;

    let body_json = row
        .get::<String>("body")
        .ok_or(JournalError::ForeignReversal {
            entry: draft_id.to_owned(),
        })?;

    let stored = serde_json::from_str::<StoredEntryDraft>(&body_json)
        .map_err(|e| JournalError::Database(cratefield_core::DbError::Query(e.to_string())))?;

    stored.into_draft()
}

/// Posts a draft: reads the draft, validates it into an `Entry`, and posts via
/// [`post_entry`] using the draft's id as `ctx.entry_id` for idempotency.
/// If the draft was already posted, a resubmit with the same `ctx.entry_id`
/// returns the original `PostedEntry`.
///
/// # Errors
/// As [`post_entry`], plus any draft lookup or validation errors.
pub async fn post_draft(
    db: &dyn Database,
    company_id: &str,
    draft_id: &str,
    ctx: &PostContext,
    policy: RetryPolicy,
    sleep_fn: impl FnMut(Duration),
    audit: &AuditContext,
) -> Result<PostedEntry, JournalError> {
    let draft = show_draft(db, company_id, draft_id).await?;
    let entry = Entry::from_draft(draft, company_id)?;

    post_entry(db, &entry, ctx, policy, sleep_fn, audit).await
}
