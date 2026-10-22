//! The account store: plain `async` functions over `&dyn Database`,
//! runnable against every backing engine (D1, SQLite, Postgres) with no
//! runtime dependency — the same shape as `ledgers-storage`'s conformance
//! suite. Queries are built with sea-query and rendered through
//! [`Statement::render`], which emits SQLite-dialect SQL with `?`
//! placeholders; the Postgres adapter rebinds them to `$n`.
//!
//! Two engine-neutral conventions, learned from the adapters:
//!
//! - `archived` crosses the wire as an `INTEGER` 0/1 bound and read as
//!   `i64`, never as a Rust `bool`: the SQLite adapter returns INTEGER
//!   columns as `BigInt` sea-query values (which `Row::get::<bool>`
//!   rejects) while Postgres returns `BOOL`/`SMALLINT` as `Bool` (which
//!   `Row::get::<i64>` rejects) — the one spelling both read back is an
//!   integer column read as an integer.
//! - `has_children` is an EXISTS-style probe implemented as
//!   `COUNT(*) … WHERE parent_number = ?`: SQLite's `count` comes back
//!   `BigInt` and Postgres's `BIGINT`, so the typed read is identical on
//!   every engine, which an `EXISTS` boolean would not be.

use crate::account::{Account, AccountEntry, AccountType, NormalBalance};
use crate::error::AccountsError;
use cratefield_core::{Database, Statement};
use ledgers_money::Currency;
use sea_query::{Alias, Expr, Query};
use std::collections::HashMap;

/// The account table name, as declared by [`crate::module::LedgerModule`].
const TABLE: &str = "account";

/// Renders the INSERT statement for one account row; the single writer
/// [`insert_account_statement`] and every batch go through, so the
/// standalone write and the batched one can never drift.
///
/// # Panics
/// When `sea_query` rejects the column/value pairing — a programmer
/// error (a column with no value, or the reverse), never a runtime
/// condition. The columns and values are listed beside each other.
#[must_use]
pub fn insert_account_statement(account: &Account) -> Statement {
    let mut insert = Query::insert();
    insert.into_table(Alias::new(TABLE)).columns([
        Alias::new("company_id"),
        Alias::new("number"),
        Alias::new("name_key"),
        Alias::new("display_name"),
        Alias::new("account_type"),
        Alias::new("normal_balance"),
        Alias::new("currency"),
        Alias::new("parent_number"),
        Alias::new("tags"),
        Alias::new("archived"),
        Alias::new("created_at"),
        Alias::new("updated_at"),
    ]);
    insert
        .values([
            account.company_id.as_str().into(),
            account.number.as_str().into(),
            account.name_key.as_str().into(),
            Expr::value(account.display_name.clone()),
            account.account_type.to_string().into(),
            account.normal_balance.to_string().into(),
            Expr::value(account.currency.map(|currency| currency.code().to_owned())),
            Expr::value(account.parent.clone()),
            join_tags(account).into(),
            sea_query::Value::BigInt(Some(i64::from(account.archived))).into(),
            account.created_at.as_str().into(),
            account.updated_at.as_str().into(),
        ])
        .expect("a value for each column, listed beside them");
    Statement::render(&insert)
}

/// Inserts one account row. No existence check here — the command layer
/// owns that (and the primary key is the final word).
///
/// # Errors
/// [`AccountsError::Database`] when the statement fails (a duplicate
/// number surfaces as a constraint error; commands pre-check and map).
pub async fn insert_account(db: &dyn Database, account: &Account) -> Result<(), AccountsError> {
    db.execute(&insert_account_statement(account))
        .await
        .map_err(constraint_on(&account.number))?;
    Ok(())
}

/// Inserts many accounts as one atomic batch (`batch_atomic` is
/// all-or-nothing on every adapter — a port contract, asserted per engine
/// by the testing kit). Used by template import; rows must already be in
/// parents-before-children order so the tree is never observed
/// half-built, and a failure part-way leaves nothing written.
///
/// # Errors
/// [`AccountsError::Database`] when the batch fails — then nothing was
/// written; [`AccountsError::AccountExists`] when the failure is a
/// primary-key conflict (mapped from the driver's constraint error).
pub async fn insert_accounts_batch(
    db: &dyn Database,
    accounts: &[Account],
) -> Result<(), AccountsError> {
    let statements = accounts
        .iter()
        .map(insert_account_statement)
        .collect::<Vec<_>>();
    db.batch_atomic(&statements)
        .await
        .map_err(|err| constraint_error(err, accounts.first().map(|a| a.number.as_str())))?;
    Ok(())
}

/// Finds one account by company and number; `None` when absent.
///
/// # Errors
/// [`AccountsError::Database`] when the query fails, and the typed-row
/// errors (never in practice — the columns are written by this crate)
/// when a stored value cannot be read back.
pub async fn find_account(
    db: &dyn Database,
    company_id: &str,
    number: &str,
) -> Result<Option<Account>, AccountsError> {
    let mut select = Query::select();
    select
        .from(Alias::new(TABLE))
        .column(Alias::new("company_id"))
        .column(Alias::new("number"))
        .column(Alias::new("name_key"))
        .column(Alias::new("display_name"))
        .column(Alias::new("account_type"))
        .column(Alias::new("normal_balance"))
        .column(Alias::new("currency"))
        .column(Alias::new("parent_number"))
        .column(Alias::new("tags"))
        .column(Alias::new("archived"))
        .column(Alias::new("created_at"))
        .column(Alias::new("updated_at"))
        .and_where(Expr::col(Alias::new("company_id")).eq(company_id))
        .and_where(Expr::col(Alias::new("number")).eq(number));
    let rows = db.query(&Statement::render(&select)).await?;
    rows.first().map(row_to_account).transpose()
}

/// Every account of the company, archived ones included, ordered by
/// number (charts use one number width, so lexical order is numeric
/// order).
///
/// # Errors
/// [`AccountsError::Database`] when the query fails or a row cannot be
/// read back.
pub async fn list_accounts(
    db: &dyn Database,
    company_id: &str,
) -> Result<Vec<Account>, AccountsError> {
    let mut select = all_columns();
    select
        .from(Alias::new(TABLE))
        .and_where(Expr::col(Alias::new("company_id")).eq(company_id))
        .order_by(Alias::new("number"), sea_query::Order::Asc);
    let rows = db.query(&Statement::render(&select)).await?;
    rows.rows.iter().map(row_to_account).collect()
}

/// Whether any account of the company names `number` as its parent: the
/// derived-postable bit. See the module docs for why this is a `COUNT`
/// probe rather than an `EXISTS` boolean.
///
/// # Errors
/// [`AccountsError::Database`] when the query fails.
pub async fn has_children(
    db: &dyn Database,
    company_id: &str,
    number: &str,
) -> Result<bool, AccountsError> {
    let mut select = Query::select();
    select
        .expr_as(Expr::cust("COUNT(*)"), Alias::new("children"))
        .from(Alias::new(TABLE))
        .and_where(Expr::col(Alias::new("company_id")).eq(company_id))
        .and_where(Expr::col(Alias::new("parent_number")).eq(number));
    let rows = db.query(&Statement::render(&select)).await?;
    let count = rows
        .first()
        .and_then(|row| row.get::<i64>("children"))
        .unwrap_or_default();
    Ok(count > 0)
}

/// Every account of the company paired with its derived has-children bit,
/// ordered by number — the read model the command surface returns. One
/// grouped query computes the bits for the whole chart; no per-account
/// probe.
///
/// # Errors
/// [`AccountsError::Database`] when a statement fails or a row cannot be
/// read back.
pub async fn list_account_entries(
    db: &dyn Database,
    company_id: &str,
) -> Result<Vec<AccountEntry>, AccountsError> {
    let accounts = list_accounts(db, company_id).await?;

    let mut counts = Query::select();
    counts
        .column(Alias::new("parent_number"))
        .expr_as(Expr::cust("COUNT(*)"), Alias::new("children"))
        .from(Alias::new(TABLE))
        .and_where(Expr::col(Alias::new("company_id")).eq(company_id))
        .and_where(Expr::col(Alias::new("parent_number")).is_not_null())
        .group_by_col(Alias::new("parent_number"));
    let rows = db.query(&Statement::render(&counts)).await?;
    let mut child_counts: HashMap<String, i64> = HashMap::with_capacity(rows.len());
    for row in &rows.rows {
        if let (Some(parent), Some(count)) = (
            row.get::<String>("parent_number"),
            row.get::<i64>("children"),
        ) {
            child_counts.insert(parent, count);
        }
    }

    Ok(accounts
        .into_iter()
        .map(|account| {
            let has_children = child_counts
                .get(&account.number)
                .is_some_and(|count| *count > 0);
            AccountEntry {
                account,
                has_children,
            }
        })
        .collect())
}

/// Renders the UPDATE that rewrites an account's mutable fields from
/// `account` (display name, class, normal balance, parent, tags) and
/// stamps `updated_at`. The number, name key, currency and archive bit
/// are not touched here: they have their own paths.
#[must_use]
pub fn update_account_statement(account: &Account) -> Statement {
    let mut update = Query::update();
    update
        .table(Alias::new(TABLE))
        .value(
            Alias::new("display_name"),
            Expr::value(account.display_name.clone()),
        )
        .value(Alias::new("account_type"), account.account_type.to_string())
        .value(
            Alias::new("normal_balance"),
            account.normal_balance.to_string(),
        )
        .value(
            Alias::new("parent_number"),
            Expr::value(account.parent.clone()),
        )
        .value(Alias::new("tags"), join_tags(account))
        .value(Alias::new("updated_at"), account.updated_at.as_str())
        .and_where(Expr::col(Alias::new("company_id")).eq(account.company_id.as_str()))
        .and_where(Expr::col(Alias::new("number")).eq(account.number.as_str()));
    Statement::render(&update)
}

/// One before/after row for a chart change: the fields the
/// `account_change` table stores (see migration 0008).
pub struct AccountChangeRow<'a> {
    /// The company the account belongs to.
    pub company_id: &'a str,
    /// The account number.
    pub number: &'a str,
    /// The command name: `account.create`, `account.update`, ...
    pub command: &'a str,
    /// Who made the change: `person:<id>` or `agent:<key id>`.
    pub actor: &'a str,
    /// The audit event the change belongs to.
    pub event_id: &'a str,
    /// When the change was made (ISO 8601 UTC).
    pub changed_at: &'a str,
    /// The snapshot before the change; `None` for a creation.
    pub before: Option<&'a serde_json::Value>,
    /// The snapshot after the change.
    pub after: &'a serde_json::Value,
}

/// The statement that writes one before/after row. Written in the change's
/// batch so the record exists exactly when the change does.
#[must_use]
pub fn account_change_statement(row: &AccountChangeRow<'_>) -> Statement {
    use sea_query::Value as SeaValue;
    Statement::with_values(
        "INSERT INTO account_change (event_id, company_id, number, command, actor, changed_at, before_json, after_json) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
            .to_owned(),
        vec![
            SeaValue::from(row.event_id),
            SeaValue::from(row.company_id),
            SeaValue::from(row.number),
            SeaValue::from(row.command),
            SeaValue::from(row.actor),
            SeaValue::from(row.changed_at),
            SeaValue::from(row.before.map(serde_json::Value::to_string)),
            SeaValue::from(row.after.to_string()),
        ],
    )
}

/// Renders the UPDATE that sets `archived` on one account, stamping
/// `updated_at`. Returned as a statement so the archive can ride the
/// same batch as its audit event; [`set_archived`] is the standalone
/// form that can still report a missing row from its affected count.
#[must_use]
pub fn set_archived_statement(
    company_id: &str,
    number: &str,
    archived: bool,
    updated_at: &str,
) -> Statement {
    let mut update = Query::update();
    update
        .table(Alias::new(TABLE))
        .value(
            Alias::new("archived"),
            sea_query::Value::BigInt(Some(i64::from(archived))),
        )
        .value(Alias::new("updated_at"), updated_at)
        .and_where(Expr::col(Alias::new("company_id")).eq(company_id))
        .and_where(Expr::col(Alias::new("number")).eq(number));
    Statement::render(&update)
}

/// Sets `archived` on one account, stamping `updated_at`. Zero rows
/// affected — the company and number match nothing — is
/// [`AccountsError::AccountNotFound`].
///
/// # Errors
/// [`AccountsError::AccountNotFound`] when no row matched;
/// [`AccountsError::Database`] when the statement fails.
pub async fn set_archived(
    db: &dyn Database,
    company_id: &str,
    number: &str,
    archived: bool,
    updated_at: &str,
) -> Result<(), AccountsError> {
    let affected = db
        .execute(&set_archived_statement(
            company_id, number, archived, updated_at,
        ))
        .await?;
    if affected == 0 {
        return Err(AccountsError::AccountNotFound {
            number: number.to_owned(),
        });
    }
    Ok(())
}

/// Deleting accounts is not an operation: posted entries are immutable
/// (docs/PLAN.md §3 rule 3) and reference accounts by number, so an
/// account must outlive every entry it ever touched. This function always
/// returns [`AccountsError::DeleteUnsupported`] without touching the
/// database — the no-delete contract, encoded in the store API so it is
/// testable and greppable rather than a convention in prose.
///
/// Deliberately `async` although it cannot await: the store surface is
/// uniformly async so callers (and the command registry) treat every
/// operation alike, and the signature survives an implementation that
/// one day records the refused attempt.
///
/// # Errors
/// Always [`AccountsError::DeleteUnsupported`].
#[allow(clippy::unused_async)]
pub async fn delete_account(
    _db: &dyn Database,
    _company_id: &str,
    _number: &str,
) -> Result<(), AccountsError> {
    Err(AccountsError::DeleteUnsupported)
}

/// Starts a select of every account column.
fn all_columns() -> sea_query::SelectStatement {
    let mut select = Query::select();
    select
        .column(Alias::new("company_id"))
        .column(Alias::new("number"))
        .column(Alias::new("name_key"))
        .column(Alias::new("display_name"))
        .column(Alias::new("account_type"))
        .column(Alias::new("normal_balance"))
        .column(Alias::new("currency"))
        .column(Alias::new("parent_number"))
        .column(Alias::new("tags"))
        .column(Alias::new("archived"))
        .column(Alias::new("created_at"))
        .column(Alias::new("updated_at"));
    select
}

/// Maps one result row to an [`Account`]. Stored strings that cannot be
/// re-typed (an account class this build does not know, a currency
/// removed from the table) surface as errors rather than silently
/// defaulted values.
fn row_to_account(row: &cratefield_core::Row) -> Result<Account, AccountsError> {
    let required = |column: &str| -> Result<String, AccountsError> {
        row.get::<String>(column).ok_or_else(|| {
            AccountsError::Database(cratefield_core::DbError::Query(format!(
                "column {column} missing or not text"
            )))
        })
    };
    let account_type: AccountType = required("account_type")?.parse()?;
    let normal_balance: NormalBalance = required("normal_balance")?.parse()?;
    // The outer Option is the port's "missing or unrepresentable"; the
    // inner one is SQL NULL (None = company base currency).
    let currency = match row.get::<Option<String>>("currency") {
        Some(Some(code)) => {
            Some(Currency::from_code(&code).map_err(|_| AccountsError::UnknownCurrency { code })?)
        }
        Some(None) => None,
        None => {
            return Err(AccountsError::Database(cratefield_core::DbError::Query(
                "column currency missing or not text".to_owned(),
            )));
        }
    };
    let tags = required("tags")?
        .split(' ')
        .filter(|tag| !tag.is_empty())
        .map(str::to_owned)
        .collect();
    let archived = row.get::<i64>("archived").ok_or_else(|| {
        AccountsError::Database(cratefield_core::DbError::Query(
            "column archived missing or not an integer".to_owned(),
        ))
    })? != 0;
    Ok(Account {
        company_id: required("company_id")?,
        number: required("number")?,
        name_key: required("name_key")?,
        display_name: row.get::<Option<String>>("display_name").flatten(),
        account_type,
        normal_balance,
        currency,
        parent: row.get::<Option<String>>("parent_number").flatten(),
        tags,
        archived,
        created_at: required("created_at")?,
        updated_at: required("updated_at")?,
    })
}

/// The tags column value: the tags joined by single spaces. Splitting on
/// the separator cannot split a tag — kebab-case tags cannot contain
/// spaces (validated before insert).
fn join_tags(account: &Account) -> String {
    account.tags.join(" ")
}

/// Whether a driver error is a uniqueness violation, best-effort across
/// engines: SQLite says `UNIQUE constraint failed: …`, Postgres
/// `duplicate key value violates unique constraint …`. Used only to pick
/// the friendlier error for a race the caller already pre-checked.
fn is_unique_violation(err: &cratefield_core::DbError) -> bool {
    let message = match err {
        cratefield_core::DbError::Execute(message)
        | cratefield_core::DbError::Query(message)
        | cratefield_core::DbError::Batch(message) => message,
    };
    let lowered = message.to_ascii_lowercase();
    lowered.contains("unique constraint") || lowered.contains("duplicate key")
}

/// Maps a constraint failure on `number` to [`AccountsError::AccountExists`],
/// passing everything else through.
fn constraint_on(number: &str) -> impl Fn(cratefield_core::DbError) -> AccountsError + '_ {
    move |err| constraint_error(err, Some(number))
}

/// The shared mapping: uniqueness failures become `AccountExists` (with
/// `number` when the caller knows which account raced), the rest pass
/// through as [`AccountsError::Database`].
fn constraint_error(err: cratefield_core::DbError, number: Option<&str>) -> AccountsError {
    if is_unique_violation(&err) {
        return AccountsError::AccountExists {
            number: number.unwrap_or_default().to_owned(),
        };
    }
    AccountsError::Database(err)
}

/// Creates a dimension value in the company.
///
/// # Errors
/// [`AccountsError::Database`] when the statement fails.
pub async fn create_dimension_value(
    db: &dyn Database,
    company_id: &str,
    kind: &str,
    code: &str,
    name: &str,
    own_books: bool,
    created_at: &str,
) -> Result<(), AccountsError> {
    use sea_query::Value as SeaValue;
    db.execute(&Statement::with_values(
        "INSERT INTO dimension_value (company_id, kind, code, name, own_books, created_at) \
         VALUES (?, ?, ?, ?, ?, ?)"
            .to_owned(),
        vec![
            SeaValue::from(company_id),
            SeaValue::from(kind),
            SeaValue::from(code),
            SeaValue::from(name),
            SeaValue::BigInt(Some(i64::from(own_books))),
            SeaValue::from(created_at),
        ],
    ))
    .await
    .map_err(AccountsError::Database)?;
    Ok(())
}

/// Sets or updates a dimension rule on an account.
///
/// # Errors
/// [`AccountsError::Database`] when the statement fails.
pub async fn set_dimension_rule(
    db: &dyn Database,
    company_id: &str,
    account_number: &str,
    kind: &str,
    rule: &str,
) -> Result<(), AccountsError> {
    use sea_query::Value as SeaValue;
    // Delete the existing rule if present, then insert (works on both SQLite and Postgres).
    db.execute(&Statement::with_values(
        "DELETE FROM account_dimension_rule \
         WHERE company_id = ? AND account_number = ? AND kind = ?"
            .to_owned(),
        vec![
            SeaValue::from(company_id),
            SeaValue::from(account_number),
            SeaValue::from(kind),
        ],
    ))
    .await
    .map_err(AccountsError::Database)?;

    db.execute(&Statement::with_values(
        "INSERT INTO account_dimension_rule (company_id, account_number, kind, rule) \
         VALUES (?, ?, ?, ?)"
            .to_owned(),
        vec![
            SeaValue::from(company_id),
            SeaValue::from(account_number),
            SeaValue::from(kind),
            SeaValue::from(rule),
        ],
    ))
    .await
    .map_err(AccountsError::Database)?;
    Ok(())
}

/// Clears a dimension rule from an account (removes the row).
///
/// # Errors
/// [`AccountsError::Database`] when the statement fails.
pub async fn clear_dimension_rule(
    db: &dyn Database,
    company_id: &str,
    account_number: &str,
    kind: &str,
) -> Result<(), AccountsError> {
    use sea_query::Value as SeaValue;
    db.execute(&Statement::with_values(
        "DELETE FROM account_dimension_rule \
         WHERE company_id = ? AND account_number = ? AND kind = ?"
            .to_owned(),
        vec![
            SeaValue::from(company_id),
            SeaValue::from(account_number),
            SeaValue::from(kind),
        ],
    ))
    .await
    .map_err(AccountsError::Database)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_round_trip_through_the_column_format() {
        let account = Account {
            company_id: "co".to_owned(),
            number: "01000".to_owned(),
            name_key: "account.t.a".to_owned(),
            display_name: None,
            account_type: AccountType::Asset,
            normal_balance: NormalBalance::Debit,
            currency: None,
            parent: None,
            tags: vec!["bank".to_owned(), "vat-input".to_owned()],
            archived: false,
            created_at: "now".to_owned(),
            updated_at: "now".to_owned(),
        };
        let joined = join_tags(&account);
        assert_eq!(joined, "bank vat-input");
        let split: Vec<String> = joined
            .split(' ')
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
            .collect();
        assert_eq!(split, account.tags);
        // Empty tag list is the empty string, not stray separators.
        assert_eq!(
            join_tags(&Account {
                tags: Vec::new(),
                ..account
            }),
            ""
        );
    }

    #[test]
    fn unique_violations_are_recognised_from_both_engines() {
        let sqlite = cratefield_core::DbError::Execute(
            "UNIQUE constraint failed: account.company_id, account.number".to_owned(),
        );
        let postgres = cratefield_core::DbError::Batch(
            "duplicate key value violates unique constraint \"account_pkey\"".to_owned(),
        );
        let other = cratefield_core::DbError::Execute("no such table: account".to_owned());
        assert!(is_unique_violation(&sqlite));
        assert!(is_unique_violation(&postgres));
        assert!(!is_unique_violation(&other));
        assert_eq!(
            constraint_error(sqlite, Some("01000")),
            AccountsError::AccountExists {
                number: "01000".to_owned()
            }
        );
        assert!(matches!(
            constraint_error(other, None),
            AccountsError::Database(_)
        ));
    }
}
