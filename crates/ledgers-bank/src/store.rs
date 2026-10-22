//! The bank rows the commands read, as typed rows. Every list is one query
//! per table, whatever the number of rows: the reads take their filters in
//! SQL, and the joins (accounts to statements, lines to candidates) are done
//! in memory over those results.

use crate::parse::CsvMapping;
use cratefield_core::{Database, DbError, Row, Statement};
use sea_query::Value;
use std::collections::HashMap;

/// The unmerged parties of `company_id` that have an IBAN, keyed by the IBAN
/// as `iban::normalise` writes it. Parties share the `vendor` table with bills.
///
/// # Errors
/// The database error when the read fails.
pub async fn parties_by_iban(
    db: &dyn Database,
    company_id: &str,
) -> Result<HashMap<String, crate::model::PartyRef>, DbError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT id, name, iban FROM vendor WHERE company_id = ? AND merged_into IS NULL AND iban IS NOT NULL ORDER BY id".to_owned(),
            vec![Value::from(company_id)],
        ))
        .await?;
    let mut by_iban = HashMap::new();
    for row in &rows.rows {
        let (Some(iban), Some(id), Some(name)) =
            (opt(row, "iban"), opt(row, "id"), opt(row, "name"))
        else {
            continue;
        };
        by_iban
            .entry(ledgers_bills::iban::normalise(&iban))
            .or_insert(crate::model::PartyRef { id, name });
    }
    Ok(by_iban)
}

/// A bank account row.
#[derive(Clone, Debug)]
pub struct AccountRow {
    /// The account id.
    pub id: String,
    /// The name.
    pub name: String,
    /// The IBAN.
    pub iban: String,
    /// The BIC.
    pub bic: Option<String>,
    /// The currency.
    pub currency: String,
    /// The linked ledger account.
    pub ledger_account_number: String,
    /// The opening balance, signed minor units.
    pub opening_minor: i64,
    /// The opening balance's date.
    pub opening_date: String,
    /// The saved CSV mapping, parsed.
    pub csv_mapping: Option<CsvMapping>,
    /// When archived.
    pub archived_at: Option<String>,
    /// Who created it.
    pub created_by: String,
    /// When created.
    pub created_at: String,
    /// When last changed.
    pub updated_at: String,
}

/// A statement row.
#[derive(Clone, Debug)]
pub struct StatementRow {
    /// The statement id.
    pub id: String,
    /// The account.
    pub account_id: String,
    /// The document it came from.
    pub document_id: String,
    /// Its place in the file.
    pub ordinal: u32,
    /// The closing date, when the file gives a closing balance.
    pub closing_date: Option<String>,
    /// The closing balance, signed minor units.
    pub closing_minor: Option<i64>,
}

/// A bank line row.
#[derive(Clone, Debug)]
pub struct TransactionRow {
    /// The line id.
    pub id: String,
    /// The account.
    pub account_id: String,
    /// The statement.
    pub statement_id: String,
    /// The fingerprint.
    pub fingerprint: String,
    /// The group key (duplicate detection).
    pub group_key: String,
    /// The booking date.
    pub booking_date: String,
    /// The value date.
    pub value_date: Option<String>,
    /// The signed amount, minor units.
    pub minor: i64,
    /// The currency.
    pub currency: String,
    /// The counterparty's name.
    pub counterparty_name: Option<String>,
    /// The counterparty's IBAN.
    pub counterparty_iban: Option<String>,
    /// The remittance.
    pub remittance: String,
    /// The bank reference.
    pub bank_reference: Option<String>,
    /// `unmatched`, `matched` or `ignored`.
    pub status: String,
    /// The matched entry.
    pub matched_entry_id: Option<String>,
    /// The matched bill.
    pub matched_bill_id: Option<String>,
}

/// A bank rule row.
#[derive(Clone, Debug)]
pub struct RuleRow {
    /// The id.
    pub id: String,
    /// The name.
    pub name: String,
    /// The conditions.
    pub conditions: crate::rules::RuleConditions,
    /// The account.
    pub account_number: String,
    /// The tax code.
    pub tax_code: Option<String>,
    /// The unit.
    pub unit: Option<String>,
    /// Whether it applies.
    pub enabled: bool,
    /// Who proposed it.
    pub proposed_by: String,
    /// Who enabled it.
    pub enabled_by: Option<String>,
    /// When created.
    pub created_at: String,
}

/// A bill that is posted and not yet settled by a bank line.
#[derive(Clone, Debug)]
pub struct OpenBillRow {
    /// The bill id.
    pub id: String,
    /// The gross total, minor units.
    pub gross_minor: i64,
    /// The vendor's name.
    pub vendor_name: String,
    /// The vendor's IBAN.
    pub vendor_iban: Option<String>,
    /// The invoice number.
    pub invoice_number: Option<String>,
    /// The invoice date.
    pub invoice_date: Option<String>,
}

/// A posted journal line on a bank's ledger account, with its entry.
#[derive(Clone, Debug)]
pub struct AccountLineRow {
    /// The ledger account number.
    pub account_number: String,
    /// The entry id.
    pub entry_id: String,
    /// The posting date.
    pub posting_date: String,
    /// The entry's description.
    pub description: String,
    /// The signed amount on the account, minor units (debit positive).
    pub minor: i64,
}

fn text(row: &Row, column: &str) -> Result<String, DbError> {
    row.get::<String>(column)
        .ok_or_else(|| DbError::Query(format!("bank row lacks {column}")))
}

fn int(row: &Row, column: &str) -> Result<i64, DbError> {
    row.get::<i64>(column)
        .ok_or_else(|| DbError::Query(format!("bank row lacks {column}")))
}

fn opt(row: &Row, column: &str) -> Option<String> {
    row.get::<Option<String>>(column).flatten()
}

/// `?, ?, ?` for `n` placeholders.
#[must_use]
pub fn placeholders(n: usize) -> String {
    vec!["?"; n].join(", ")
}

const ACCOUNT_COLUMNS: &str = "id, name, iban, bic, currency, ledger_account_number, opening_minor, \
     opening_date, csv_mapping, archived_at, created_by, created_at, updated_at";

fn account_from(row: &Row) -> Result<AccountRow, DbError> {
    let csv_mapping = opt(row, "csv_mapping")
        .map(|json| serde_json::from_str::<CsvMapping>(&json))
        .transpose()
        .map_err(|err| DbError::Query(format!("bank account mapping: {err}")))?;
    Ok(AccountRow {
        id: text(row, "id")?,
        name: text(row, "name")?,
        iban: text(row, "iban")?,
        bic: opt(row, "bic"),
        currency: text(row, "currency")?,
        ledger_account_number: text(row, "ledger_account_number")?,
        opening_minor: int(row, "opening_minor")?,
        opening_date: text(row, "opening_date")?,
        csv_mapping,
        archived_at: opt(row, "archived_at"),
        created_by: text(row, "created_by")?,
        created_at: text(row, "created_at")?,
        updated_at: text(row, "updated_at")?,
    })
}

/// One account, by id.
///
/// # Errors
/// A database error or a row that does not decode.
pub async fn account(
    db: &dyn Database,
    company_id: &str,
    id: &str,
) -> Result<Option<AccountRow>, DbError> {
    let stmt = Statement::with_values(
        format!("SELECT {ACCOUNT_COLUMNS} FROM bank_account WHERE company_id = ? AND id = ?"),
        vec![Value::from(company_id), Value::from(id)],
    );
    db.query(&stmt).await?.first().map(account_from).transpose()
}

/// The company's accounts, by name.
///
/// # Errors
/// A database error or a row that does not decode.
pub async fn accounts(
    db: &dyn Database,
    company_id: &str,
    include_archived: bool,
) -> Result<Vec<AccountRow>, DbError> {
    let archived = if include_archived {
        ""
    } else {
        " AND archived_at IS NULL"
    };
    let stmt = Statement::with_values(
        format!(
            "SELECT {ACCOUNT_COLUMNS} FROM bank_account WHERE company_id = ?{archived} ORDER BY name, id"
        ),
        vec![Value::from(company_id)],
    );
    db.query(&stmt)
        .await?
        .rows
        .iter()
        .map(account_from)
        .collect()
}

/// Every statement of the given accounts, in file order.
///
/// # Errors
/// A database error or a row that does not decode.
pub async fn statements_of(
    db: &dyn Database,
    company_id: &str,
    account_ids: &[String],
) -> Result<Vec<StatementRow>, DbError> {
    if account_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut values = vec![Value::from(company_id)];
    values.extend(account_ids.iter().map(|id| Value::from(id.as_str())));
    let stmt = Statement::with_values(
        format!(
            "SELECT id, account_id, document_id, ordinal, closing_date, closing_minor FROM bank_statement \
             WHERE company_id = ? AND account_id IN ({}) ORDER BY account_id, closing_date, ordinal",
            placeholders(account_ids.len())
        ),
        values,
    );
    db.query(&stmt)
        .await?
        .rows
        .iter()
        .map(|row| {
            Ok(StatementRow {
                id: text(row, "id")?,
                account_id: text(row, "account_id")?,
                document_id: text(row, "document_id")?,
                ordinal: u32::try_from(int(row, "ordinal")?).unwrap_or(0),
                closing_date: opt(row, "closing_date"),
                closing_minor: row.get::<i64>("closing_minor"),
            })
        })
        .collect()
}

/// The statements already imported for an account and document, as
/// `(ordinal, statement id)`.
///
/// # Errors
/// A database error.
pub async fn imported_statements(
    db: &dyn Database,
    company_id: &str,
    account_id: &str,
    document_id: &str,
) -> Result<Vec<(u32, String)>, DbError> {
    let stmt = Statement::with_values(
        "SELECT id, ordinal FROM bank_statement WHERE company_id = ? AND account_id = ? AND document_id = ?"
            .to_owned(),
        vec![Value::from(company_id), Value::from(account_id), Value::from(document_id)],
    );
    let rows = db.query(&stmt).await?;
    rows.rows
        .iter()
        .map(|row| {
            Ok((
                u32::try_from(int(row, "ordinal")?).unwrap_or(0),
                text(row, "id")?,
            ))
        })
        .collect()
}

/// How many stored lines each group key has on an account.
///
/// # Errors
/// A database error.
pub async fn group_counts(
    db: &dyn Database,
    company_id: &str,
    account_id: &str,
) -> Result<HashMap<String, u32>, DbError> {
    let stmt = Statement::with_values(
        "SELECT group_key, COUNT(*) AS n FROM bank_transaction WHERE company_id = ? AND account_id = ? \
         GROUP BY group_key"
            .to_owned(),
        vec![Value::from(company_id), Value::from(account_id)],
    );
    let rows = db.query(&stmt).await?;
    rows.rows
        .iter()
        .map(|row| {
            Ok((
                text(row, "group_key")?,
                u32::try_from(int(row, "n")?).unwrap_or(u32::MAX),
            ))
        })
        .collect()
}

/// Line counts per account and status.
///
/// # Errors
/// A database error.
pub async fn status_counts(
    db: &dyn Database,
    company_id: &str,
) -> Result<HashMap<(String, String), u32>, DbError> {
    let stmt = Statement::with_values(
        "SELECT account_id, status, COUNT(*) AS n FROM bank_transaction WHERE company_id = ? \
         GROUP BY account_id, status"
            .to_owned(),
        vec![Value::from(company_id)],
    );
    let rows = db.query(&stmt).await?;
    rows.rows
        .iter()
        .map(|row| {
            Ok((
                (text(row, "account_id")?, text(row, "status")?),
                u32::try_from(int(row, "n")?).unwrap_or(u32::MAX),
            ))
        })
        .collect()
}

/// The filter of a line list, already validated by the command.
#[derive(Clone, Debug, Default)]
pub struct TransactionFilter {
    /// One account.
    pub account_id: Option<String>,
    /// Only this stored status (`unmatched`, `matched` or `ignored`).
    pub status: Option<String>,
    /// Lowercase text the remittance, name or IBAN contains.
    pub search: Option<String>,
    /// Booking date on or after.
    pub from: Option<String>,
    /// Booking date on or before.
    pub to: Option<String>,
    /// Keyset cursor: `booking_date|id` of the last line of the previous page.
    pub after: Option<(String, String)>,
    /// Rows to read (the page size plus one, to know whether there is more).
    pub take: u32,
}

const TRANSACTION_COLUMNS: &str = "id, account_id, statement_id, fingerprint, group_key, booking_date, \
     value_date, minor, currency, counterparty_name, counterparty_iban, remittance, bank_reference, status, \
     matched_entry_id, matched_bill_id";

fn transaction_from(row: &Row) -> Result<TransactionRow, DbError> {
    Ok(TransactionRow {
        id: text(row, "id")?,
        account_id: text(row, "account_id")?,
        statement_id: text(row, "statement_id")?,
        fingerprint: text(row, "fingerprint")?,
        group_key: text(row, "group_key")?,
        booking_date: text(row, "booking_date")?,
        value_date: opt(row, "value_date"),
        minor: int(row, "minor")?,
        currency: text(row, "currency")?,
        counterparty_name: opt(row, "counterparty_name"),
        counterparty_iban: opt(row, "counterparty_iban"),
        remittance: text(row, "remittance")?,
        bank_reference: opt(row, "bank_reference"),
        status: text(row, "status")?,
        matched_entry_id: opt(row, "matched_entry_id"),
        matched_bill_id: opt(row, "matched_bill_id"),
    })
}

/// One page of lines, newest first.
///
/// # Errors
/// A database error or a row that does not decode.
pub async fn transactions(
    db: &dyn Database,
    company_id: &str,
    filter: &TransactionFilter,
) -> Result<Vec<TransactionRow>, DbError> {
    let mut sql =
        format!("SELECT {TRANSACTION_COLUMNS} FROM bank_transaction WHERE company_id = ?");
    let mut values = vec![Value::from(company_id)];
    if let Some(account) = &filter.account_id {
        sql.push_str(" AND account_id = ?");
        values.push(Value::from(account.as_str()));
    }
    if let Some(status) = &filter.status {
        sql.push_str(" AND status = ?");
        values.push(Value::from(status.as_str()));
    }
    if let Some(from) = &filter.from {
        sql.push_str(" AND booking_date >= ?");
        values.push(Value::from(from.as_str()));
    }
    if let Some(to) = &filter.to {
        sql.push_str(" AND booking_date <= ?");
        values.push(Value::from(to.as_str()));
    }
    if let Some(search) = &filter.search {
        let pattern = format!("%{search}%");
        sql.push_str(
            " AND (LOWER(remittance) LIKE ? OR LOWER(COALESCE(counterparty_name, '')) LIKE ? \
             OR LOWER(COALESCE(counterparty_iban, '')) LIKE ?)",
        );
        for _ in 0..3 {
            values.push(Value::from(pattern.as_str()));
        }
    }
    if let Some((date, id)) = &filter.after {
        sql.push_str(" AND (booking_date < ? OR (booking_date = ? AND id < ?))");
        values.push(Value::from(date.as_str()));
        values.push(Value::from(date.as_str()));
        values.push(Value::from(id.as_str()));
    }
    sql.push_str(" ORDER BY booking_date DESC, id DESC LIMIT ?");
    values.push(Value::from(i64::from(filter.take)));
    let rows = db.query(&Statement::with_values(sql, values)).await?;
    rows.rows.iter().map(transaction_from).collect()
}

/// One line, by id.
///
/// # Errors
/// A database error or a row that does not decode.
pub async fn transaction(
    db: &dyn Database,
    company_id: &str,
    id: &str,
) -> Result<Option<TransactionRow>, DbError> {
    let stmt = Statement::with_values(
        format!(
            "SELECT {TRANSACTION_COLUMNS} FROM bank_transaction WHERE company_id = ? AND id = ?"
        ),
        vec![Value::from(company_id), Value::from(id)],
    );
    db.query(&stmt)
        .await?
        .first()
        .map(transaction_from)
        .transpose()
}

const RULE_COLUMNS: &str = "id, name, contains_text, counterparty_iban, amount_min, amount_max, \
     account_number, tax_code, unit, enabled, proposed_by, enabled_by, created_at";

fn rule_from(row: &Row) -> Result<RuleRow, DbError> {
    Ok(RuleRow {
        id: text(row, "id")?,
        name: text(row, "name")?,
        conditions: crate::rules::RuleConditions {
            contains: opt(row, "contains_text"),
            counterparty_iban: opt(row, "counterparty_iban"),
            amount_min: row.get::<i64>("amount_min"),
            amount_max: row.get::<i64>("amount_max"),
        },
        account_number: text(row, "account_number")?,
        tax_code: opt(row, "tax_code"),
        unit: opt(row, "unit"),
        enabled: int(row, "enabled")? != 0,
        proposed_by: text(row, "proposed_by")?,
        enabled_by: opt(row, "enabled_by"),
        created_at: text(row, "created_at")?,
    })
}

/// The company's rules, oldest first. With `enabled_only`, only those that apply.
///
/// # Errors
/// A database error or a row that does not decode.
pub async fn rules(
    db: &dyn Database,
    company_id: &str,
    enabled_only: bool,
) -> Result<Vec<RuleRow>, DbError> {
    let filter = if enabled_only { " AND enabled = 1" } else { "" };
    let stmt = Statement::with_values(
        format!(
            "SELECT {RULE_COLUMNS} FROM bank_rule WHERE company_id = ?{filter} ORDER BY created_at, id"
        ),
        vec![Value::from(company_id)],
    );
    db.query(&stmt).await?.rows.iter().map(rule_from).collect()
}

/// One rule, by id.
///
/// # Errors
/// A database error or a row that does not decode.
pub async fn rule(
    db: &dyn Database,
    company_id: &str,
    id: &str,
) -> Result<Option<RuleRow>, DbError> {
    let stmt = Statement::with_values(
        format!("SELECT {RULE_COLUMNS} FROM bank_rule WHERE company_id = ? AND id = ?"),
        vec![Value::from(company_id), Value::from(id)],
    );
    db.query(&stmt).await?.first().map(rule_from).transpose()
}

/// Posted bills in a currency that no bank line has settled yet.
///
/// # Errors
/// A database error or a row that does not decode.
pub async fn open_bills(
    db: &dyn Database,
    company_id: &str,
    currency: &str,
) -> Result<Vec<OpenBillRow>, DbError> {
    let stmt = Statement::with_values(
        "SELECT b.id, b.gross_minor, b.invoice_number, b.invoice_date, v.name AS vendor_name, \
         v.iban AS vendor_iban FROM bill b JOIN vendor v ON v.id = b.vendor_id \
         WHERE b.company_id = ? AND b.currency = ? AND b.posted_entry_id IS NOT NULL \
         AND b.discarded_at IS NULL AND b.gross_minor IS NOT NULL \
         AND NOT EXISTS (SELECT 1 FROM bank_transaction t WHERE t.company_id = b.company_id \
         AND t.matched_bill_id = b.id) ORDER BY b.id"
            .to_owned(),
        vec![Value::from(company_id), Value::from(currency)],
    );
    let rows = db.query(&stmt).await?;
    rows.rows
        .iter()
        .map(|row| {
            Ok(OpenBillRow {
                id: text(row, "id")?,
                gross_minor: int(row, "gross_minor")?,
                vendor_name: text(row, "vendor_name")?,
                vendor_iban: opt(row, "vendor_iban"),
                invoice_number: opt(row, "invoice_number"),
                invoice_date: opt(row, "invoice_date"),
            })
        })
        .collect()
}

/// The most recent posted lines on the given ledger accounts that no bank
/// line has been matched to, at most `limit` per account.
///
/// # Errors
/// A database error or a row that does not decode.
pub async fn account_lines(
    db: &dyn Database,
    company_id: &str,
    numbers: &[String],
    limit: u32,
) -> Result<Vec<AccountLineRow>, DbError> {
    if numbers.is_empty() {
        return Ok(Vec::new());
    }
    let mut values = vec![Value::from(company_id)];
    values.extend(numbers.iter().map(|n| Value::from(n.as_str())));
    values.push(Value::from(company_id));
    values.push(Value::from(
        i64::from(limit) * i64::try_from(numbers.len()).unwrap_or(i64::MAX),
    ));
    let stmt = Statement::with_values(
        format!(
            "SELECT l.account_number, e.id AS entry_id, e.posting_date, e.description, l.base_minor \
             FROM journal_line l JOIN journal_entry e ON e.id = l.entry_id \
             WHERE e.company_id = ? AND l.account_number IN ({}) \
             AND NOT EXISTS (SELECT 1 FROM bank_transaction t WHERE t.company_id = ? AND t.matched_entry_id = e.id) \
             ORDER BY e.posting_date DESC, e.id DESC LIMIT ?",
            placeholders(numbers.len())
        ),
        values,
    );
    let rows = db.query(&stmt).await?;
    rows.rows
        .iter()
        .map(|row| {
            Ok(AccountLineRow {
                account_number: text(row, "account_number")?,
                entry_id: text(row, "entry_id")?,
                posting_date: text(row, "posting_date")?,
                description: text(row, "description")?,
                minor: int(row, "base_minor")?,
            })
        })
        .collect()
}

/// Every posted line on the given ledger accounts, with its posting date,
/// for the balance as of a date. One query for all accounts.
///
/// # Errors
/// A database error or a row that does not decode.
pub async fn ledger_lines(
    db: &dyn Database,
    company_id: &str,
    numbers: &[String],
) -> Result<Vec<(String, String, i64)>, DbError> {
    if numbers.is_empty() {
        return Ok(Vec::new());
    }
    let mut values = vec![Value::from(company_id)];
    values.extend(numbers.iter().map(|n| Value::from(n.as_str())));
    let stmt = Statement::with_values(
        format!(
            "SELECT l.account_number, e.posting_date, l.base_minor FROM journal_line l \
             JOIN journal_entry e ON e.id = l.entry_id \
             WHERE e.company_id = ? AND l.account_number IN ({})",
            placeholders(numbers.len())
        ),
        values,
    );
    let rows = db.query(&stmt).await?;
    rows.rows
        .iter()
        .map(|row| {
            Ok((
                text(row, "account_number")?,
                text(row, "posting_date")?,
                int(row, "base_minor")?,
            ))
        })
        .collect()
}
