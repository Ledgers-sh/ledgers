//! Read queries over posted entries and account balances: what the
//! command surface lists and fetches (`entries.list`, `entries.get`,
//! `accounts.list` balances). Reads observe; nothing here writes, and
//! every query is scoped to one company.

use crate::entry::{Entry, JournalError, Line};
use crate::reversal::read_entry_with_lines;
use cratefield_core::{Database, Statement};
use ledgers_money::{Amount, Currency};
use sea_query::Value as SeaValue;

/// One posted entry, as the books hold it: the header, its lines, and
/// the entries that reversed it.
#[derive(Clone, Debug)]
pub struct EntryRecord {
    /// The entry's id (its `request_id` at posting).
    pub id: String,
    /// The fiscal year its number is scoped to.
    pub fiscal_year: i64,
    /// The gapless number within the journal and fiscal year.
    pub number: i64,
    /// The recorded-at timestamp (ISO 8601 UTC).
    pub recorded_at: String,
    /// Who posted it: `agent` or `human`, as the journal recorded it.
    pub actor: String,
    /// The entry itself: header fields and validated lines.
    pub entry: Entry,
    /// The lines, in posting order.
    pub lines: Vec<Line>,
    /// Ids of the entries that reverse this one (empty when none).
    pub reversed_by: Vec<String>,
}

/// The balance of one account in one base currency, debit-positive, over
/// every posted line of the company.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountBalance {
    /// The account number.
    pub account_number: String,
    /// The base currency the balance is in.
    pub currency: Currency,
    /// The signed balance (debit positive).
    pub balance: Amount,
}

/// Reads one posted entry by its id, scoped to `company_id`.
///
/// # Errors
/// [`JournalError::Database`] when a query fails.
pub async fn entry_by_id(
    db: &dyn Database,
    company_id: &str,
    id: &str,
) -> Result<Option<EntryRecord>, JournalError> {
    let header = db
        .query(&Statement::with_values(
            "SELECT id, fiscal_year, number, recorded_at, external_id, description, actor FROM journal_entry \
             WHERE id = ? AND company_id = ?"
                .to_owned(),
            vec![SeaValue::from(id), SeaValue::from(company_id)],
        ))
        .await?;
    let Some(row) = header.first() else {
        return Ok(None);
    };
    let fiscal_year = row.get::<i64>("fiscal_year").unwrap_or_default();
    let number = row.get::<i64>("number").unwrap_or_default();
    let recorded_at = row.get::<String>("recorded_at").unwrap_or_default();
    let actor = row
        .get::<String>("actor")
        .unwrap_or_else(|| "agent".to_owned());

    let (mut entry, lines) = read_entry_with_lines(db, company_id, id).await?;
    // The header row is the source of the idempotency key; the reader does not load it.
    entry.external_id = row.get::<Option<String>>("external_id").flatten();
    entry.description = row.get::<String>("description").unwrap_or_default();
    let reversed_by = crate::entry::reversed_by(db, company_id, id).await?;
    Ok(Some(EntryRecord {
        id: id.to_owned(),
        fiscal_year,
        number,
        recorded_at,
        actor,
        entry,
        lines,
        reversed_by,
    }))
}

/// Reads one posted entry by the `external_id` its source application
/// gave it at posting (the idempotency key of the source system).
///
/// # Errors
/// [`JournalError::Database`] when a query fails.
pub async fn entry_by_external_id(
    db: &dyn Database,
    company_id: &str,
    external_id: &str,
) -> Result<Option<EntryRecord>, JournalError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT id FROM journal_entry WHERE company_id = ? AND external_id = ?".to_owned(),
            vec![SeaValue::from(company_id), SeaValue::from(external_id)],
        ))
        .await?;
    let Some(id) = rows.first().and_then(|row| row.get::<String>("id")) else {
        return Ok(None);
    };
    entry_by_id(db, company_id, &id).await
}

/// Lists posted entries of one VAT period (`YYYY-MM`) or of every period,
/// newest posting first, at most `limit` of them.
///
/// # Errors
/// [`JournalError::Database`] when a query fails.
pub async fn list_entries(
    db: &dyn Database,
    company_id: &str,
    period: Option<&str>,
    limit: u32,
) -> Result<Vec<EntryRecord>, JournalError> {
    let rows = match period {
        Some(period) => {
            db.query(&Statement::with_values(
                "SELECT id FROM journal_entry WHERE company_id = ? AND vat_period = ? \
                 ORDER BY posting_date DESC, fiscal_year DESC, number DESC LIMIT ?"
                    .to_owned(),
                vec![
                    SeaValue::from(company_id),
                    SeaValue::from(period),
                    SeaValue::from(i64::from(limit)),
                ],
            ))
            .await?
        }
        None => {
            db.query(&Statement::with_values(
                "SELECT id FROM journal_entry WHERE company_id = ? \
                 ORDER BY posting_date DESC, fiscal_year DESC, number DESC LIMIT ?"
                    .to_owned(),
                vec![SeaValue::from(company_id), SeaValue::from(i64::from(limit))],
            ))
            .await?
        }
    };
    let mut records = Vec::with_capacity(rows.rows.len());
    for row in &rows.rows {
        let Some(id) = row.get::<String>("id") else {
            continue;
        };
        if let Some(record) = entry_by_id(db, company_id, &id).await? {
            records.push(record);
        }
    }
    Ok(records)
}

/// The balance of every account that has posted lines, one row per
/// account and base currency, debit-positive.
///
/// # Errors
/// [`JournalError::Database`] when the query fails.
pub async fn account_balances(
    db: &dyn Database,
    company_id: &str,
) -> Result<Vec<AccountBalance>, JournalError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT jl.account_number AS account_number, je.base_currency AS base_currency, \
             COALESCE(SUM(jl.base_minor), 0) AS total \
             FROM journal_line jl JOIN journal_entry je ON jl.entry_id = je.id \
             WHERE je.company_id = ? \
             GROUP BY jl.account_number, je.base_currency \
             ORDER BY jl.account_number, je.base_currency"
                .to_owned(),
            vec![SeaValue::from(company_id)],
        ))
        .await?;
    let mut balances = Vec::with_capacity(rows.rows.len());
    for row in &rows.rows {
        let account_number: String = row
            .get("account_number")
            .ok_or_else(|| JournalError::Database(missing_column("account_number")))?;
        let code: String = row
            .get("base_currency")
            .ok_or_else(|| JournalError::Database(missing_column("base_currency")))?;
        let total: i64 = row
            .get("total")
            .ok_or_else(|| JournalError::Database(missing_column("total")))?;
        let currency = Currency::from_code(&code)?;
        balances.push(AccountBalance {
            account_number,
            currency,
            balance: Amount::from_i64(total, currency),
        });
    }
    Ok(balances)
}

fn missing_column(name: &str) -> cratefield_core::DbError {
    cratefield_core::DbError::Query(format!("query result has no column {name}"))
}
