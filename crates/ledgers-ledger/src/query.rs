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
             CAST(COALESCE(SUM(jl.base_minor), 0) AS BIGINT) AS total \
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

/// The base currencies in which an account (bound twice: company, number)
/// nets to a non-zero balance: no row means the account is settled. The
/// archive refusal reads this, and the archive batch re-proves it with a
/// guard built from the same clause (`NOT EXISTS (<this>)`).
pub const NONZERO_BALANCE_SQL: &str = "SELECT je.base_currency FROM journal_line jl \
     JOIN journal_entry je ON jl.entry_id = je.id \
     WHERE je.company_id = ? AND jl.account_number = ? \
     GROUP BY je.base_currency HAVING SUM(jl.base_minor) <> 0";

/// Whether any posted line names `account_number` (bound twice: company,
/// number). Accounts with postings keep their class and cannot take
/// children or a new parent.
pub const POSTING_EXISTS_SQL: &str = "SELECT jl.entry_id FROM journal_line jl \
     JOIN journal_entry je ON jl.entry_id = je.id \
     WHERE je.company_id = ? AND jl.account_number = ? LIMIT 1";

/// Whether an account of the company has any posted line.
///
/// # Errors
/// [`JournalError::Database`] when the query fails.
pub async fn account_has_postings(
    db: &dyn Database,
    company_id: &str,
    number: &str,
) -> Result<bool, cratefield_core::DbError> {
    Ok(
        !company_number_rows(db, POSTING_EXISTS_SQL, company_id, number)
            .await?
            .is_empty(),
    )
}

/// Whether an account of the company nets to a non-zero balance in some
/// base currency.
///
/// # Errors
/// [`cratefield_core::DbError`] when the query fails.
pub async fn account_has_nonzero_balance(
    db: &dyn Database,
    company_id: &str,
    number: &str,
) -> Result<bool, cratefield_core::DbError> {
    Ok(
        !company_number_rows(db, NONZERO_BALANCE_SQL, company_id, number)
            .await?
            .is_empty(),
    )
}

/// Runs a two-placeholder (company, number) SELECT and returns its rows.
async fn company_number_rows(
    db: &dyn Database,
    sql: &str,
    company_id: &str,
    number: &str,
) -> Result<cratefield_core::Rows, cratefield_core::DbError> {
    db.query(&Statement::with_values(
        sql.to_owned(),
        vec![SeaValue::from(company_id), SeaValue::from(number)],
    ))
    .await
}

/// One posted line of an account, as the account page lists it: the entry
/// it belongs to and the account's signed base amount in that entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountActivity {
    /// The entry's id (its `request_id` at posting).
    pub entry_id: String,
    /// The journal the entry posted into.
    pub journal_id: String,
    /// The fiscal year the entry number is scoped to.
    pub fiscal_year: i64,
    /// The gapless number within the journal and fiscal year.
    pub number: i64,
    /// The accounting date (ISO 8601 date).
    pub posting_date: String,
    /// The entry's description.
    pub description: String,
    /// The account's signed base amount in this entry (debit positive).
    pub base: Amount,
}

/// The most recent posted entries touching one account, newest first, at
/// most `limit` of them. Reverses appear as their own entries: history is
/// shown, never rewritten.
///
/// # Errors
/// [`cratefield_core::DbError`] when the query fails, or the currency of a
/// stored row cannot be read back.
pub async fn account_activity(
    db: &dyn Database,
    company_id: &str,
    account_number: &str,
    limit: u32,
) -> Result<Vec<AccountActivity>, JournalError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT je.id AS id, je.journal_id AS journal_id, je.fiscal_year AS fiscal_year, \
             je.number AS number, je.posting_date AS posting_date, je.description AS description, \
             je.base_currency AS base_currency, CAST(COALESCE(SUM(jl.base_minor), 0) AS BIGINT) AS total \
             FROM journal_line jl JOIN journal_entry je ON jl.entry_id = je.id \
             WHERE je.company_id = ? AND jl.account_number = ? \
             GROUP BY je.id, je.journal_id, je.fiscal_year, je.number, je.posting_date, \
             je.description, je.base_currency \
             ORDER BY je.posting_date DESC, je.fiscal_year DESC, je.number DESC LIMIT ?"
                .to_owned(),
            vec![
                SeaValue::from(company_id),
                SeaValue::from(account_number),
                SeaValue::from(i64::from(limit)),
            ],
        ))
        .await?;
    let mut activity = Vec::with_capacity(rows.rows.len());
    for row in &rows.rows {
        let code: String = row
            .get("base_currency")
            .ok_or_else(|| JournalError::Database(missing_column("base_currency")))?;
        let total: i64 = row
            .get("total")
            .ok_or_else(|| JournalError::Database(missing_column("total")))?;
        let currency = Currency::from_code(&code)?;
        activity.push(AccountActivity {
            entry_id: row
                .get("id")
                .ok_or_else(|| JournalError::Database(missing_column("id")))?,
            journal_id: row
                .get("journal_id")
                .ok_or_else(|| JournalError::Database(missing_column("journal_id")))?,
            fiscal_year: row
                .get("fiscal_year")
                .ok_or_else(|| JournalError::Database(missing_column("fiscal_year")))?,
            number: row
                .get("number")
                .ok_or_else(|| JournalError::Database(missing_column("number")))?,
            posting_date: row
                .get("posting_date")
                .ok_or_else(|| JournalError::Database(missing_column("posting_date")))?,
            description: row
                .get("description")
                .ok_or_else(|| JournalError::Database(missing_column("description")))?,
            base: Amount::from_i64(total, currency),
        });
    }
    Ok(activity)
}
