//! The group rows: the reads the commands and reports make, and the typed
//! rows they are built from. Reads are scoped by the id they are given; the
//! server decides which companies a person may see before a view is built.

use crate::model::{
    Accounts, AgreementView, Basis, DocStatus, Kind, PeriodDocView, Side, WhtBasis,
};
use cratefield_core::{Database, DbError, Row, Statement};
use sea_query::Value;

/// One group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupRow {
    /// The group id.
    pub id: String,
    /// The group name.
    pub name: String,
    /// Who created it.
    pub created_by: String,
    /// When created.
    pub created_at: String,
    /// When last changed.
    pub updated_at: String,
}

/// One member of a group, as its own company's row holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberRow {
    /// The group.
    pub group_id: String,
    /// The member company.
    pub company_id: String,
    /// The kind text.
    pub kind: String,
    /// The ownership the group holds in the member, in basis points.
    pub ownership_bp: i64,
}

/// One agreement row: one side of one agreement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgreementRow {
    /// The company whose row this is.
    pub company_id: String,
    /// The view of the row.
    pub view: AgreementView,
    /// The other company's id, always known to the store.
    pub counterparty_id: String,
    /// The group id.
    pub group_id: String,
    /// The rate, the fixed amount and the currency, as the row stores them.
    pub fixed_minor: Option<i64>,
    /// The base currency of the agreement.
    pub currency: String,
}

/// One period document row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocRow {
    /// The company whose row this is.
    pub company_id: String,
    /// The agreement.
    pub agreement_id: String,
    /// The period document.
    pub view: PeriodDocView,
    /// The post attempt the row last started.
    pub attempt: i64,
}

/// The columns of an agreement read, in the order [`agreement_from_row`] reads.
pub const AGREEMENT_SELECT: &str = "SELECT company_id, id, group_id, side, counterparty_id, kind, basis, rate_bp, \
     fixed_minor, currency, wht_bp, wht_basis, treaty_override_bp, account_royalty, account_ic, account_wht, \
     account_bank, account_unit, notes, terminated_at, created_by, created_at, updated_at FROM intercompany_agreement";

/// The columns of a document read.
pub const DOC_SELECT: &str = "SELECT company_id, id, side, agreement_id, period, status, currency, basis_minor, \
     gross_minor, wht_minor, net_minor, attempt, accrual_entry_id, settle_entry_id, last_error \
     FROM intercompany_document";

fn text(row: &Row, column: &str) -> Result<String, DbError> {
    row.get::<String>(column)
        .ok_or_else(|| DbError::Query(format!("group row lacks {column}")))
}

fn optional(row: &Row, column: &str) -> Option<String> {
    row.get::<Option<String>>(column).flatten()
}

fn integer(row: &Row, column: &str) -> Result<i64, DbError> {
    row.get::<i64>(column)
        .ok_or_else(|| DbError::Query(format!("group row lacks {column}")))
}

fn bad(column: &str, value: &str) -> DbError {
    DbError::Query(format!("group row has an unknown {column}: {value:?}"))
}

/// Decodes one agreement row.
///
/// # Errors
/// A column is missing or holds a value this build does not know.
pub fn agreement_from_row(row: &Row) -> Result<AgreementRow, DbError> {
    let side_text = text(row, "side")?;
    let side = Side::parse(&side_text).ok_or_else(|| bad("side", &side_text))?;
    let kind_text = text(row, "kind")?;
    let kind = Kind::parse(&kind_text).ok_or_else(|| bad("kind", &kind_text))?;
    let basis_text = text(row, "basis")?;
    let basis = Basis::parse(&basis_text).ok_or_else(|| bad("basis", &basis_text))?;
    let wht_text = text(row, "wht_basis")?;
    let wht_basis = WhtBasis::parse(&wht_text).ok_or_else(|| bad("wht_basis", &wht_text))?;
    let counterparty_id = text(row, "counterparty_id")?;
    let group_id = text(row, "group_id")?;
    let currency = text(row, "currency")?;
    let fixed_minor = row.get::<i64>("fixed_minor");
    let terminated_at = optional(row, "terminated_at");
    let view = AgreementView {
        id: text(row, "id")?,
        group_id: group_id.clone(),
        side,
        counterparty_id: Some(counterparty_id.clone()),
        counterparty_hidden: false,
        kind,
        basis,
        rate_bp: row.get::<i64>("rate_bp"),
        fixed_minor,
        currency: currency.clone(),
        wht_bp: integer(row, "wht_bp")?,
        wht_basis,
        treaty_override_bp: row.get::<i64>("treaty_override_bp"),
        accounts: Accounts {
            royalty: text(row, "account_royalty")?,
            ic: text(row, "account_ic")?,
            wht: optional(row, "account_wht"),
            bank: text(row, "account_bank")?,
            unit: optional(row, "account_unit"),
        },
        notes: optional(row, "notes"),
        terminated_at,
        created_by: text(row, "created_by")?,
        created_at: text(row, "created_at")?,
        updated_at: text(row, "updated_at")?,
    };
    Ok(AgreementRow {
        company_id: text(row, "company_id")?,
        view,
        counterparty_id,
        group_id,
        fixed_minor,
        currency,
    })
}

/// Decodes one document row.
///
/// # Errors
/// A column is missing or holds a value this build does not know.
pub fn doc_from_row(row: &Row) -> Result<DocRow, DbError> {
    let side_text = text(row, "side")?;
    let side = Side::parse(&side_text).ok_or_else(|| bad("side", &side_text))?;
    let status_text = text(row, "status")?;
    let status = DocStatus::parse(&status_text).ok_or_else(|| bad("status", &status_text))?;
    Ok(DocRow {
        company_id: text(row, "company_id")?,
        agreement_id: text(row, "agreement_id")?,
        attempt: integer(row, "attempt")?,
        view: PeriodDocView {
            id: text(row, "id")?,
            side,
            period: text(row, "period")?,
            status,
            currency: text(row, "currency")?,
            basis_minor: integer(row, "basis_minor")?,
            gross_minor: integer(row, "gross_minor")?,
            wht_minor: integer(row, "wht_minor")?,
            net_minor: integer(row, "net_minor")?,
            accrual_entry_id: optional(row, "accrual_entry_id"),
            settle_entry_id: optional(row, "settle_entry_id"),
            last_error: optional(row, "last_error"),
        },
    })
}

/// The groups a company is a member of, with their names.
///
/// # Errors
/// A database error or a row that does not decode.
pub async fn groups_of_company(
    db: &dyn Database,
    company_id: &str,
) -> Result<Vec<GroupRow>, DbError> {
    let stmt = Statement::with_values(
        "SELECT g.id, g.name, g.created_by, g.created_at, g.updated_at FROM company_group g \
         JOIN company_group_member m ON m.group_id = g.id \
         WHERE m.company_id = ? ORDER BY g.name, g.id"
            .to_owned(),
        vec![Value::from(company_id)],
    );
    let rows = db.query(&stmt).await?;
    rows.rows
        .iter()
        .map(|row| {
            Ok(GroupRow {
                id: text(row, "id")?,
                name: text(row, "name")?,
                created_by: text(row, "created_by")?,
                created_at: text(row, "created_at")?,
                updated_at: text(row, "updated_at")?,
            })
        })
        .collect()
}

/// One group by id.
///
/// # Errors
/// A database error.
pub async fn group_by_id(db: &dyn Database, group_id: &str) -> Result<Option<GroupRow>, DbError> {
    let stmt = Statement::with_values(
        "SELECT id, name, created_by, created_at, updated_at FROM company_group WHERE id = ?"
            .to_owned(),
        vec![Value::from(group_id)],
    );
    let rows = db.query(&stmt).await?;
    rows.rows
        .first()
        .map(|row| {
            Ok(GroupRow {
                id: text(row, "id")?,
                name: text(row, "name")?,
                created_by: text(row, "created_by")?,
                created_at: text(row, "created_at")?,
                updated_at: text(row, "updated_at")?,
            })
        })
        .transpose()
}

/// Every member of `group_id`, from every member's partition. The caller
/// decides which of them the person may see.
///
/// # Errors
/// A database error.
pub async fn members_of(db: &dyn Database, group_id: &str) -> Result<Vec<MemberRow>, DbError> {
    members_of_groups(db, &[group_id.to_owned()]).await
}

/// Every member of the given groups, in one query.
///
/// # Errors
/// A database error.
pub async fn members_of_groups(
    db: &dyn Database,
    group_ids: &[String],
) -> Result<Vec<MemberRow>, DbError> {
    if group_ids.is_empty() {
        return Ok(Vec::new());
    }
    let marks = vec!["?"; group_ids.len()].join(", ");
    let stmt = Statement::with_values(
        format!(
            "SELECT group_id, company_id, kind, ownership_bp FROM company_group_member \
             WHERE group_id IN ({marks}) ORDER BY group_id, company_id"
        ),
        group_ids
            .iter()
            .map(|id| Value::from(id.as_str()))
            .collect(),
    );
    let rows = db.query(&stmt).await?;
    rows.rows
        .iter()
        .map(|row| {
            Ok(MemberRow {
                group_id: text(row, "group_id")?,
                company_id: text(row, "company_id")?,
                kind: text(row, "kind")?,
                ownership_bp: integer(row, "ownership_bp")?,
            })
        })
        .collect()
}

/// Both sides of one agreement: the licensor's row and the licensee's row,
/// whichever companies they are in. Empty when no such agreement exists.
///
/// # Errors
/// A database error or a row that does not decode.
pub async fn agreement_sides(
    db: &dyn Database,
    agreement_id: &str,
) -> Result<Vec<AgreementRow>, DbError> {
    let stmt = Statement::with_values(
        format!("{AGREEMENT_SELECT} WHERE id = ? ORDER BY side"),
        vec![Value::from(agreement_id)],
    );
    let rows = db.query(&stmt).await?;
    rows.rows.iter().map(agreement_from_row).collect()
}

/// The agreement rows a company holds (its own side of each agreement), in
/// one query.
///
/// # Errors
/// A database error or a row that does not decode.
pub async fn agreements_of_company(
    db: &dyn Database,
    company_id: &str,
) -> Result<Vec<AgreementRow>, DbError> {
    let stmt = Statement::with_values(
        format!("{AGREEMENT_SELECT} WHERE company_id = ? ORDER BY created_at, id"),
        vec![Value::from(company_id)],
    );
    let rows = db.query(&stmt).await?;
    rows.rows.iter().map(agreement_from_row).collect()
}

/// The document rows of one agreement, both sides, in period order, in one
/// query.
///
/// # Errors
/// A database error or a row that does not decode.
pub async fn docs_of_agreement(
    db: &dyn Database,
    agreement_id: &str,
) -> Result<Vec<DocRow>, DbError> {
    let stmt = Statement::with_values(
        format!("{DOC_SELECT} WHERE agreement_id = ? ORDER BY period, side"),
        vec![Value::from(agreement_id)],
    );
    let rows = db.query(&stmt).await?;
    rows.rows.iter().map(doc_from_row).collect()
}

/// The document rows of one agreement for one period, both sides.
///
/// # Errors
/// A database error or a row that does not decode.
pub async fn docs_for_period(
    db: &dyn Database,
    agreement_id: &str,
    period: &str,
) -> Result<Vec<DocRow>, DbError> {
    let stmt = Statement::with_values(
        format!("{DOC_SELECT} WHERE agreement_id = ? AND period = ? ORDER BY side"),
        vec![Value::from(agreement_id), Value::from(period)],
    );
    let rows = db.query(&stmt).await?;
    rows.rows.iter().map(doc_from_row).collect()
}

/// The country pack id a company set (`country_pack`), if any.
///
/// # Errors
/// A database error.
pub async fn country_pack_of(
    db: &dyn Database,
    company_id: &str,
) -> Result<Option<String>, DbError> {
    let stmt = Statement::with_values(
        "SELECT value FROM company_setting WHERE company_id = ? AND key = 'country_pack'"
            .to_owned(),
        vec![Value::from(company_id)],
    );
    let rows = db.query(&stmt).await?;
    Ok(rows.rows.first().and_then(|row| row.get::<String>("value")))
}

/// The base currency of a company.
///
/// # Errors
/// A database error, or a company with no row.
pub async fn base_currency_of(
    db: &dyn Database,
    company_id: &str,
) -> Result<Option<String>, DbError> {
    let stmt = Statement::with_values(
        "SELECT base_currency FROM company WHERE id = ?".to_owned(),
        vec![Value::from(company_id)],
    );
    let rows = db.query(&stmt).await?;
    Ok(rows
        .rows
        .first()
        .and_then(|row| row.get::<String>("base_currency")))
}

/// The names of the given companies, in one query, keyed by id.
///
/// # Errors
/// A database error.
pub async fn company_names(
    db: &dyn Database,
    company_ids: &[String],
) -> Result<std::collections::BTreeMap<String, String>, DbError> {
    if company_ids.is_empty() {
        return Ok(std::collections::BTreeMap::new());
    }
    let marks = vec!["?"; company_ids.len()].join(", ");
    let stmt = Statement::with_values(
        format!("SELECT id, name FROM company WHERE id IN ({marks})"),
        company_ids
            .iter()
            .map(|id| Value::from(id.as_str()))
            .collect(),
    );
    let rows = db.query(&stmt).await?;
    let mut out = std::collections::BTreeMap::new();
    for row in &rows.rows {
        out.insert(text(row, "id")?, text(row, "name")?);
    }
    Ok(out)
}

/// The name of a company, if it exists.
///
/// # Errors
/// A database error.
pub async fn company_name(db: &dyn Database, company_id: &str) -> Result<Option<String>, DbError> {
    let stmt = Statement::with_values(
        "SELECT name FROM company WHERE id = ?".to_owned(),
        vec![Value::from(company_id)],
    );
    let rows = db.query(&stmt).await?;
    Ok(rows.rows.first().and_then(|row| row.get::<String>("name")))
}

/// The ledger amount of the lines an agreement's entries posted to `account`:
/// the debit-positive sum over entries whose `external_id` starts with the
/// agreement's prefix, and over the entries that reverse them.
///
/// # Errors
/// A database error.
pub async fn agreement_balance(
    db: &dyn Database,
    company_id: &str,
    agreement_id: &str,
    account: &str,
) -> Result<i64, DbError> {
    let prefix = format!("ic-{agreement_id}-%");
    let stmt = Statement::with_values(
        "SELECT CAST(COALESCE(SUM(jl.base_minor), 0) AS BIGINT) AS total FROM journal_line jl \
         JOIN journal_entry je ON je.id = jl.entry_id \
         WHERE je.company_id = ? AND jl.account_number = ? AND (je.external_id LIKE ? OR je.reverses IN \
         (SELECT r.id FROM journal_entry r WHERE r.company_id = ? AND r.external_id LIKE ?))"
            .to_owned(),
        vec![
            Value::from(company_id),
            Value::from(account),
            Value::from(prefix.as_str()),
            Value::from(company_id),
            Value::from(prefix.as_str()),
        ],
    );
    let rows = db.query(&stmt).await?;
    Ok(rows
        .rows
        .first()
        .and_then(|row| row.get::<i64>("total"))
        .unwrap_or(0))
}
