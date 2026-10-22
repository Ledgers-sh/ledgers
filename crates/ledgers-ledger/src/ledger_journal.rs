//! The production [`Journal`] port: [`EntryJournal`] runs the fiscal
//! machinery's posts through the #67 posting batch ([`crate::posting`])
//! over the `journal`, `journal_entry` and `journal_line` tables.
//!
//! What the port asks for maps onto the batch like this:
//!
//! - **`post`** goes through [`posting::post_raw_entry_with`]: numbering,
//!   lines, the journal-ownership, period-open (or person-side
//!   period-postable, for `human` posts) and accounts guards, the balanced
//!   guard, the daily-allowance consume, the audit append and the outbox
//!   enqueue all commit or refuse together. A closed period refuses for
//!   everyone; a `soft-closed` period refuses agent keys and admits people.
//! - **`balances`** and **`find_entry`** read the posted rows back.
//! - **`draft_count`** is always `0`: #67 has no draft state (every entry
//!   is posted on write), so there is nothing to count.
//! - **`base_currency`** is the currency of the company, stored in the
//!   company table. A port post takes its currency from its lines, which
//!   must agree.

use crate::entry::JournalError;
use crate::error::AccountsError;
use crate::journal::{JournalDraft, JournalKind, create_journal, get_journal};
use crate::journal_port::{AccountBalance, BoxFuture, Journal, NewEntry, PostedEntry};
use crate::posting::{self, PostContext, PostOptions, RawEntry, RawLine};
use cratefield_core::{Database, DbError, Statement};
use ledgers_audit::AuditContext;
use ledgers_db::retry::RetryPolicy;
use ledgers_money::{Amount, Currency};
use sea_query::Value as SeaValue;
use std::collections::BTreeMap;

/// The production implementation of the [`Journal`] port. Stateless: the
/// database handle arrives with each call, as the port requires.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EntryJournal;

/// The suffix the closing-entry outbox id carries, distinct from the
/// entry id so an event row never collides with its entry's key.
const OUTBOX_SUFFIX: &str = ":outbox";

impl Journal for EntryJournal {
    fn base_currency<'a>(
        &'a self,
        db: &'a dyn Database,
        company_id: &'a str,
    ) -> BoxFuture<'a, Result<Currency, AccountsError>> {
        Box::pin(async move { base_currency_of(db, company_id).await })
    }

    fn balances<'a>(
        &'a self,
        db: &'a dyn Database,
        company_id: &'a str,
    ) -> BoxFuture<'a, Result<Vec<AccountBalance>, AccountsError>> {
        Box::pin(async move {
            let base = base_currency_of(db, company_id).await?;
            let rows = db
                .query(&Statement::with_values(
                    "SELECT a.number AS account_number, CAST(COALESCE((\n\
                         SELECT SUM(l.base_minor) FROM journal_line l\n\
                         JOIN journal_entry e ON e.id = l.entry_id\n\
                         WHERE e.company_id = a.company_id AND l.account_number = a.number\n\
                     ), 0) AS BIGINT) AS balance\n\
                     FROM account a\n\
                     WHERE a.company_id = ?\n\
                     AND EXISTS (\n\
                         SELECT 1 FROM journal_line l\n\
                         JOIN journal_entry e ON e.id = l.entry_id\n\
                         WHERE e.company_id = a.company_id AND l.account_number = a.number\n\
                     )\n\
                     ORDER BY a.number"
                        .to_owned(),
                    vec![SeaValue::from(company_id)],
                ))
                .await?;
            let mut balances = Vec::with_capacity(rows.rows.len());
            for row in &rows.rows {
                let account_number = row.get::<String>("account_number").ok_or_else(|| {
                    DbError::Query("account_number missing or not text".to_owned())
                })?;
                let minor = row
                    .get::<i64>("balance")
                    .ok_or_else(|| DbError::Query("balance missing or not BIGINT".to_owned()))?;
                balances.push(AccountBalance {
                    account_number,
                    balance: Amount::from_i64(minor, base),
                });
            }
            Ok(balances)
        })
    }

    fn draft_count<'a>(
        &'a self,
        _db: &'a dyn Database,
        _company_id: &'a str,
        _from: &'a str,
        _to: &'a str,
    ) -> BoxFuture<'a, Result<i64, AccountsError>> {
        // #67 has no draft state: every entry is posted on write.
        Box::pin(async { Ok(0) })
    }

    fn post<'a>(
        &'a self,
        db: &'a dyn Database,
        entry: &'a NewEntry,
        audit: &'a AuditContext,
    ) -> BoxFuture<'a, Result<PostedEntry, AccountsError>> {
        Box::pin(async move { post_new_entry(db, entry, audit).await })
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
                    "SELECT number, posting_date FROM journal_entry\n\
                     WHERE company_id = ? AND id = ?"
                        .to_owned(),
                    vec![SeaValue::from(company_id), SeaValue::from(entry_id)],
                ))
                .await?;
            let Some(row) = rows.rows.first() else {
                return Ok(None);
            };
            let number = row
                .get::<i64>("number")
                .ok_or_else(|| DbError::Query("entry number missing or not BIGINT".to_owned()))?;
            let entry_date = row
                .get::<String>("posting_date")
                .ok_or_else(|| DbError::Query("posting_date missing or not text".to_owned()))?;
            Ok(Some(PostedEntry {
                entry_id: entry_id.to_owned(),
                number,
                entry_date,
            }))
        })
    }

    fn ensure_journal<'a>(
        &'a self,
        db: &'a dyn Database,
        company_id: &'a str,
        journal_id: &'a str,
        code: &'a str,
        name: &'a str,
        created_at: &'a str,
        audit: &'a AuditContext,
    ) -> BoxFuture<'a, Result<(), AccountsError>> {
        Box::pin(async move {
            if get_journal(db, journal_id)
                .await
                .map_err(into_accounts)?
                .is_some()
            {
                return Ok(());
            }
            let draft = JournalDraft {
                code: code.to_owned(),
                kind: JournalKind::General,
                name: name.to_owned(),
            };
            create_journal(db, company_id, draft, journal_id, created_at, audit)
                .await
                .map(|_| ())
                .map_err(into_accounts)
        })
    }
}

/// The base currency of a company, read from the company configuration.
async fn base_currency_of(db: &dyn Database, company_id: &str) -> Result<Currency, AccountsError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT base_currency FROM company WHERE id = ?".to_owned(),
            vec![SeaValue::from(company_id)],
        ))
        .await?;
    let code = rows
        .rows
        .first()
        .and_then(|row| row.get::<String>("base_currency"))
        .ok_or_else(|| {
            DbError::Query(format!(
                "company {company_id} has no base currency configured"
            ))
        })?;
    Ok(code.parse::<Currency>()?)
}

/// Posts one port entry through the batch and reads it back as the port's
/// [`PostedEntry`].
async fn post_new_entry(
    db: &dyn Database,
    entry: &NewEntry,
    audit: &AuditContext,
) -> Result<PostedEntry, AccountsError> {
    let journal = get_journal(db, &entry.journal_id)
        .await
        .map_err(into_accounts)?
        .ok_or_else(|| {
            AccountsError::Database(DbError::Query(format!(
                "journal {} does not exist; ensure_journal provisions it first",
                entry.journal_id
            )))
        })?;
    // The entry's currency is the one its lines carry, as on the entries
    // API (whose drafts name their base currency per entry); a mixed-currency
    // entry is refused here, before the batch.
    let base = match entry.lines.first() {
        Some(first) => first.amount.currency(),
        None => {
            return Err(AccountsError::Database(DbError::Query(
                "an entry needs at least two lines".to_owned(),
            )));
        }
    };
    let mut lines = Vec::with_capacity(entry.lines.len());
    for line in &entry.lines {
        if line.amount.currency() != base {
            return Err(AccountsError::Database(DbError::Query(format!(
                "line on {} is not in the entry's currency {}",
                line.account_number,
                base.code()
            ))));
        }
        let minor = i64::try_from(line.amount.minor()).map_err(|_| {
            AccountsError::Database(DbError::Query(format!(
                "line on {} overflows the storage width",
                line.account_number
            )))
        })?;
        lines.push(RawLine {
            account_number: line.account_number.clone(),
            base_currency: base.code().to_owned(),
            base_minor: minor,
            tx_currency: base.code().to_owned(),
            tx_minor: minor,
            rate_scaled: None,
            tax_code: None,
            dimensions: BTreeMap::new(),
            memo: None,
        });
    }
    let raw = RawEntry {
        company_id: journal.company_id.clone(),
        journal_id: entry.journal_id.clone(),
        document_date: entry.entry_date.clone(),
        posting_date: entry.entry_date.clone(),
        tax_point_date: entry.entry_date.clone(),
        base_currency: base.code().to_owned(),
        description: entry.memo.clone(),
        source_documents: Vec::new(),
        reverses: None,
        external_id: None,
        lines,
    };
    let ctx = PostContext {
        fiscal_year: entry.fiscal_year,
        entry_id: entry.entry_id.clone(),
        recorded_at: entry.created_at.clone(),
        outbox_id: format!("{}{OUTBOX_SUFFIX}", entry.entry_id),
    };
    let options = PostOptions {
        period_override: entry.period.clone(),
        human: entry.human,
        daily_limit: Some((entry.actor_subject.clone(), entry.daily_limit)),
    };
    let posted = posting::post_raw_entry_with(
        db,
        &raw,
        &ctx,
        RetryPolicy::default(),
        |_| {},
        &options,
        audit,
    )
    .await
    .map_err(into_accounts)?;
    Ok(PostedEntry {
        entry_id: posted.entry_id,
        number: posted.number,
        entry_date: entry.entry_date.clone(),
    })
}

/// Maps the entries API's error onto the port's. Every refusal (closed
/// period, unbalanced entry, spent allowance, foreign journal) is a
/// database refusal on this port, as the port's contract documents;
/// contention keeps its own variant.
fn into_accounts(err: JournalError) -> AccountsError {
    match err {
        JournalError::Conflict { attempts } => AccountsError::Conflict { attempts },
        JournalError::Money(money) => AccountsError::Money(money),
        JournalError::Database(db) => AccountsError::Database(db),
        other => AccountsError::Database(DbError::Query(other.to_string())),
    }
}
