//! Reporting module for trial balance, profit & loss, balance sheet,
//! and dimension-based analysis.
//!
//! This module provides functions to generate key financial reports by querying
//! the journal entries and aggregating balances by account, period, and dimension.
//! All queries operate over immutable posted entries (docs/PLAN.md §3 rule 3).

use cratefield_core::{Database, DbError, Statement};
use ledgers_money::{Amount, Currency};
use sea_query::Value as SeaValue;
use std::collections::BTreeMap;
use std::str::FromStr;

use crate::entry::JournalError;
use crate::error::AccountsError;
use crate::units::UnitScope;

/// The marker a scoped SQL text carries where the unit condition goes:
/// after the WHERE conditions, before GROUP BY.
const SCOPE_MARK: &str = "/*scope*/";

/// A statement whose SQL carries [`SCOPE_MARK`]: the scope's condition is
/// spliced in there and its values are appended after the query's own.
fn scoped_statement(
    scope: &UnitScope,
    sql: impl Into<String>,
    mut values: Vec<SeaValue>,
) -> Statement {
    let sql: String = sql.into();
    debug_assert!(
        sql.contains(SCOPE_MARK),
        "a scoped query carries the scope marker"
    );
    let (filter, extra) = scope.line_filter();
    values.extend(extra);
    Statement::with_values(sql.replace(SCOPE_MARK, &filter), values)
}

/// Helper to convert missing column to [`JournalError`]
fn required_column(column: &str) -> JournalError {
    JournalError::Database(DbError::Query(format!(
        "column {column} missing or not the expected type"
    )))
}

/// An account balance in a trial balance report: account code and debit/credit sides.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TrialBalanceRow {
    /// The account's number in the chart.
    pub account_code: String,
    /// Total debits in the period (positive amounts).
    pub debit: Amount,
    /// Total credits in the period (positive amounts).
    pub credit: Amount,
    /// Balance (debit-positive signed amount).
    pub balance: Amount,
}

/// Profit and loss statement: revenue, expenses, and net income for a period.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ProfitAndLoss {
    /// Total revenue (sum of all income account balances).
    pub revenue: Amount,
    /// Total expenses (sum of all expense account balances).
    pub expenses: Amount,
    /// Net income: revenue minus expenses.
    pub net_income: Amount,
}

/// Balance sheet: assets, liabilities, and equity as of a date.
///
/// Equity is presented the standard way: the equity accounts plus a
/// "current year result" line (Jahresüberschuss / Jahresfehlbetrag), which is
/// income minus expenses from the fiscal year start up to the as-of date. The
/// year's P&L accounts are not closed into retained earnings until the
/// year-end close, so without that line the sheet would not balance while the
/// year has activity. Once the year is closed, its closing entry nets the P&L
/// accounts to zero, so the line is zero and the result is not counted twice.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BalanceSheet {
    /// Total assets (sum of asset account balances).
    pub assets: Amount,
    /// Total liabilities (sum of liability account balances).
    pub liabilities: Amount,
    /// Total equity as presented: `equity_accounts` plus `current_year_result`.
    pub equity: Amount,
    /// The equity accounts alone (capital, reserves, retained earnings).
    pub equity_accounts: Amount,
    /// The current year result: income minus expenses from the fiscal year
    /// start up to the as-of date. Negative for a loss.
    pub current_year_result: Amount,
    /// Assets minus (liabilities plus equity). Zero when the sheet balances.
    pub difference: Amount,
    /// Whether `difference` is zero.
    pub balanced: bool,
}

/// One dimension value's balance for an account.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DimensionBalance {
    /// The dimension value (e.g., cost centre code or customer id).
    pub dimension_value: String,
    /// Balance for this dimension (debit-positive signed amount).
    pub balance: Amount,
}

/// The company's base currency, read from the company row. Reports never
/// infer it from posted entries: a company with no entries has one too.
///
/// # Errors
/// [`JournalError::Database`] when the query fails or the company has no
/// base currency; [`JournalError::Money`] when the stored code is not a currency.
pub async fn company_currency(
    db: &dyn Database,
    company_id: &str,
) -> Result<Currency, JournalError> {
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
            JournalError::Database(DbError::Query(format!(
                "company {company_id} has no base currency configured"
            )))
        })?;
    Ok(Currency::from_str(&code)?)
}

/// Generates a trial balance for the given period: all accounts with their
/// debit and credit totals, and the resulting balance.
///
/// # Arguments
/// * `db` - The database connection
/// * `company_id` - The company identifier
/// * `period` - The period in "YYYY-MM" format
///
/// # Errors
/// Returns [`JournalError::Database`] if the query fails.
///
/// # Returns
/// A vector of trial balance rows, one per account that had activity in the period.
pub async fn trial_balance(
    db: &dyn Database,
    company_id: &str,
    period: &str,
) -> Result<Vec<TrialBalanceRow>, JournalError> {
    trial_balance_scoped(db, company_id, period, &UnitScope::Consolidated).await
}

/// [`trial_balance`] for one reporting scope: the company or one business unit.
///
/// # Errors
/// As [`trial_balance`].
pub async fn trial_balance_scoped(
    db: &dyn Database,
    company_id: &str,
    period: &str,
    scope: &UnitScope,
) -> Result<Vec<TrialBalanceRow>, JournalError> {
    let sql = r"
        SELECT
            jl.account_number,
            je.base_currency,
            CAST(COALESCE(SUM(CASE WHEN jl.base_minor > 0 THEN jl.base_minor ELSE 0 END), 0) AS BIGINT) as total_debits,
            CAST(COALESCE(SUM(CASE WHEN jl.base_minor < 0 THEN -jl.base_minor ELSE 0 END), 0) AS BIGINT) as total_credits
        FROM journal_line jl
        JOIN journal_entry je ON jl.entry_id = je.id
        WHERE je.company_id = ? AND je.vat_period = ?
        /*scope*/
        GROUP BY jl.account_number, je.base_currency
        ORDER BY jl.account_number
    ";

    let statement = scoped_statement(
        scope,
        sql.to_string(),
        vec![SeaValue::from(company_id), SeaValue::from(period)],
    );

    let rows = db.query(&statement).await?;
    let mut result = Vec::new();

    for row in &rows.rows {
        let account_code: String = row
            .get("account_number")
            .ok_or_else(|| required_column("account_number"))?;
        let currency_code: String = row
            .get("base_currency")
            .ok_or_else(|| required_column("base_currency"))?;
        let debit_minor: i64 = row
            .get("total_debits")
            .ok_or_else(|| required_column("total_debits"))?;
        let credit_minor: i64 = row
            .get("total_credits")
            .ok_or_else(|| required_column("total_credits"))?;

        let currency = Currency::from_str(&currency_code)?;
        let debit = Amount::from_i64(debit_minor, currency);
        let credit = Amount::from_i64(credit_minor, currency);
        let balance = Amount::from_i64(debit_minor - credit_minor, currency);

        result.push(TrialBalanceRow {
            account_code,
            debit,
            credit,
            balance,
        });
    }

    Ok(result)
}

/// Generates a trial balance filtered by a dimension value (e.g., for a specific unit or cost centre).
///
/// # Arguments
/// * `db` - The database connection
/// * `company_id` - The company identifier
/// * `period` - The period in "YYYY-MM" format
/// * `dimension_kind` - The dimension type ("unit", "`cost_centre`", "project")
/// * `dimension_value` - The specific dimension value to filter by
///
/// # Errors
/// Returns [`JournalError::Database`] if the query fails.
///
/// # Returns
/// A vector of trial balance rows for lines tagged with the specified dimension,
/// one per account that had activity with that dimension in the period.
pub async fn trial_balance_for_dimension(
    db: &dyn Database,
    company_id: &str,
    period: &str,
    dimension_kind: &str,
    dimension_value: &str,
) -> Result<Vec<TrialBalanceRow>, JournalError> {
    let sql = r"
        SELECT
            jl.account_number,
            je.base_currency,
            CAST(COALESCE(SUM(CASE WHEN jl.base_minor > 0 THEN jl.base_minor ELSE 0 END), 0) AS BIGINT) as total_debits,
            CAST(COALESCE(SUM(CASE WHEN jl.base_minor < 0 THEN -jl.base_minor ELSE 0 END), 0) AS BIGINT) as total_credits
        FROM journal_line jl
        JOIN journal_entry je ON jl.entry_id = je.id
        JOIN journal_line_dimension jld ON jl.entry_id = jld.entry_id AND jl.line_no = jld.line_no
        WHERE je.company_id = ? AND je.vat_period = ? AND jld.kind = ? AND jld.value = ?
        GROUP BY jl.account_number, je.base_currency
        ORDER BY jl.account_number
    ";

    let statement = Statement::with_values(
        sql.to_string(),
        vec![
            SeaValue::from(company_id),
            SeaValue::from(period),
            SeaValue::from(dimension_kind),
            SeaValue::from(dimension_value),
        ],
    );

    let rows = db.query(&statement).await?;
    let mut result = Vec::new();

    for row in &rows.rows {
        let account_code: String = row
            .get("account_number")
            .ok_or_else(|| required_column("account_number"))?;
        let currency_code: String = row
            .get("base_currency")
            .ok_or_else(|| required_column("base_currency"))?;
        let debit_minor: i64 = row
            .get("total_debits")
            .ok_or_else(|| required_column("total_debits"))?;
        let credit_minor: i64 = row
            .get("total_credits")
            .ok_or_else(|| required_column("total_credits"))?;

        let currency = Currency::from_str(&currency_code)?;
        let debit = Amount::from_i64(debit_minor, currency);
        let credit = Amount::from_i64(credit_minor, currency);
        let balance = Amount::from_i64(debit_minor - credit_minor, currency);

        result.push(TrialBalanceRow {
            account_code,
            debit,
            credit,
            balance,
        });
    }

    Ok(result)
}

/// Generates a profit and loss statement for the given period.
///
/// # Arguments
/// * `db` - The database connection
/// * `company_id` - The company identifier
/// * `period` - The period in "YYYY-MM" format
///
/// # Errors
/// Returns [`JournalError::Database`] if the query fails.
///
/// # Returns
/// A [`ProfitAndLoss`] struct with revenue, expenses, and net income.
pub async fn profit_and_loss(
    db: &dyn Database,
    company_id: &str,
    period: &str,
) -> Result<ProfitAndLoss, JournalError> {
    profit_and_loss_scoped(db, company_id, period, &UnitScope::Consolidated).await
}

/// [`profit_and_loss`] for one reporting scope.
///
/// # Errors
/// As [`profit_and_loss`].
pub async fn profit_and_loss_scoped(
    db: &dyn Database,
    company_id: &str,
    period: &str,
    scope: &UnitScope,
) -> Result<ProfitAndLoss, JournalError> {
    let sql = r"
        SELECT
            a.account_type,
            CAST(COALESCE(SUM(jl.base_minor), 0) AS BIGINT) as total_amount
        FROM journal_line jl
        JOIN journal_entry je ON jl.entry_id = je.id
        JOIN account a ON jl.account_number = a.number AND je.company_id = a.company_id
        WHERE je.company_id = ? AND je.vat_period = ? AND a.account_type IN ('income', 'expense')
        /*scope*/
        GROUP BY a.account_type
    ";

    let statement = scoped_statement(
        scope,
        sql.to_string(),
        vec![SeaValue::from(company_id), SeaValue::from(period)],
    );

    let rows = db.query(&statement).await?;

    let mut revenue_minor = 0i64;
    let mut expenses_minor = 0i64;

    for row in &rows.rows {
        let account_type: String = row
            .get("account_type")
            .ok_or_else(|| required_column("account_type"))?;
        let total_amount: i64 = row
            .get("total_amount")
            .ok_or_else(|| required_column("total_amount"))?;

        if account_type == "income" {
            // Income is credit-normal, so negate to get the P&L value
            revenue_minor -= total_amount;
        } else if account_type == "expense" {
            // Expenses are debit-normal, use as-is
            expenses_minor += total_amount;
        }
    }

    let currency = company_currency(db, company_id).await?;

    let revenue = Amount::from_i64(revenue_minor, currency);
    let expenses = Amount::from_i64(expenses_minor, currency);
    let net_income = Amount::from_i64(revenue_minor - expenses_minor, currency);

    Ok(ProfitAndLoss {
        revenue,
        expenses,
        net_income,
    })
}

/// The start of the fiscal year that contains `as_of_date`: the `starts_on`
/// of the covering `fiscal_year` row. Without a covering row the calendar year
/// starts the year, which is what a company without a fiscal year calendar
/// means by one.
async fn fiscal_year_start(
    db: &dyn Database,
    company_id: &str,
    as_of_date: &str,
) -> Result<String, JournalError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT starts_on FROM fiscal_year \
             WHERE company_id = ? AND starts_on <= ? AND ends_on >= ? \
             ORDER BY starts_on DESC LIMIT 1"
                .to_owned(),
            vec![
                SeaValue::from(company_id),
                SeaValue::from(as_of_date),
                SeaValue::from(as_of_date),
            ],
        ))
        .await?;
    match rows.rows.first() {
        Some(row) => row
            .get("starts_on")
            .ok_or_else(|| required_column("starts_on")),
        None => Ok(format!(
            "{}-01-01",
            as_of_date.get(..4).unwrap_or(as_of_date)
        )),
    }
}

/// Generates a balance sheet as of the given date: cumulative balances
/// from all entries up to and including that date, with the current year
/// result added to equity (see [`BalanceSheet`]).
///
/// # Arguments
/// * `db` - The database connection
/// * `company_id` - The company identifier
/// * `as_of_date` - The date in "YYYY-MM-DD" format
///
/// # Errors
/// Returns [`JournalError::Database`] if the query fails.
///
/// # Returns
/// A `BalanceSheet` struct with assets, liabilities, equity and the balance check.
pub async fn balance_sheet(
    db: &dyn Database,
    company_id: &str,
    as_of_date: &str,
) -> Result<BalanceSheet, JournalError> {
    balance_sheet_scoped(db, company_id, as_of_date, &UnitScope::Consolidated).await
}

/// [`balance_sheet`] for one reporting scope.
///
/// # Errors
/// As [`balance_sheet`].
pub async fn balance_sheet_scoped(
    db: &dyn Database,
    company_id: &str,
    as_of_date: &str,
    scope: &UnitScope,
) -> Result<BalanceSheet, JournalError> {
    let sql = r"
        SELECT
            a.account_type,
            CAST(COALESCE(SUM(jl.base_minor), 0) AS BIGINT) as total_amount
        FROM journal_line jl
        JOIN journal_entry je ON jl.entry_id = je.id
        JOIN account a ON jl.account_number = a.number AND je.company_id = a.company_id
        WHERE je.company_id = ? AND je.posting_date <= ? AND a.account_type IN ('asset', 'liability', 'equity')
        /*scope*/
        GROUP BY a.account_type
    ";

    let statement = scoped_statement(
        scope,
        sql.to_string(),
        vec![SeaValue::from(company_id), SeaValue::from(as_of_date)],
    );

    let rows = db.query(&statement).await?;

    let mut assets_minor = 0i64;
    let mut liabilities_minor = 0i64;
    let mut equity_minor = 0i64;

    for row in &rows.rows {
        let account_type: String = row
            .get("account_type")
            .ok_or_else(|| required_column("account_type"))?;
        let total_amount: i64 = row
            .get("total_amount")
            .ok_or_else(|| required_column("total_amount"))?;

        match account_type.as_str() {
            "asset" => assets_minor += total_amount,
            "liability" => liabilities_minor -= total_amount, // Credit-normal
            "equity" => equity_minor -= total_amount,         // Credit-normal
            _ => {}
        }
    }

    let currency = company_currency(db, company_id).await?;

    // The current year result: income and expense activity from the fiscal
    // year start up to the as-of date. Income is credit-normal, so the
    // equity-side result is the negated sum of the P&L balances. A closed year
    // carries a closing entry that nets its P&L accounts to zero, so a closed
    // year contributes nothing here: its result already sits in retained
    // earnings.
    let year_start = fiscal_year_start(db, company_id, as_of_date).await?;
    let pnl_sql = r"
        SELECT CAST(COALESCE(SUM(jl.base_minor), 0) AS BIGINT) as total_amount
        FROM journal_line jl
        JOIN journal_entry je ON jl.entry_id = je.id
        JOIN account a ON jl.account_number = a.number AND je.company_id = a.company_id
        WHERE je.company_id = ? AND je.posting_date >= ? AND je.posting_date <= ?
          AND a.account_type IN ('income', 'expense')
          /*scope*/
    ";
    let pnl_rows = db
        .query(&scoped_statement(
            scope,
            pnl_sql.to_string(),
            vec![
                SeaValue::from(company_id),
                SeaValue::from(year_start.as_str()),
                SeaValue::from(as_of_date),
            ],
        ))
        .await?;
    let pnl_debit_minor: i64 = match pnl_rows.rows.first() {
        Some(row) => row
            .get("total_amount")
            .ok_or_else(|| required_column("total_amount"))?,
        None => 0,
    };
    let current_year_result_minor = -pnl_debit_minor;

    let equity_total_minor = equity_minor + current_year_result_minor;
    let difference_minor = assets_minor - liabilities_minor - equity_total_minor;

    let assets = Amount::from_i64(assets_minor, currency);
    let liabilities = Amount::from_i64(liabilities_minor, currency);
    let equity = Amount::from_i64(equity_total_minor, currency);

    Ok(BalanceSheet {
        assets,
        liabilities,
        equity,
        equity_accounts: Amount::from_i64(equity_minor, currency),
        current_year_result: Amount::from_i64(current_year_result_minor, currency),
        difference: Amount::from_i64(difference_minor, currency),
        balanced: difference_minor == 0,
    })
}

/// Breaks down an account balance by dimension (e.g., cost centre, customer).
///
/// # Arguments
/// * `db` - The database connection
/// * `company_id` - The company identifier
/// * `account_code` - The account number to analyze
/// * `dimension_key` - The dimension to aggregate by (e.g., "cost-centre", "customer")
/// * `period` - The period in "YYYY-MM" format
///
/// # Errors
/// Returns [`JournalError::Database`] if the query fails.
///
/// # Returns
/// A vector of dimension balance rows, one per dimension value found for the account in the period.
pub async fn account_balance_by_dimension(
    db: &dyn Database,
    company_id: &str,
    account_code: &str,
    dimension_key: &str,
    period: &str,
) -> Result<Vec<DimensionBalance>, JournalError> {
    account_balance_by_dimension_scoped(
        db,
        company_id,
        account_code,
        dimension_key,
        period,
        &UnitScope::Consolidated,
    )
    .await
}

/// [`account_balance_by_dimension`] for one reporting scope.
///
/// # Errors
/// As [`account_balance_by_dimension`].
pub async fn account_balance_by_dimension_scoped(
    db: &dyn Database,
    company_id: &str,
    account_code: &str,
    dimension_key: &str,
    period: &str,
    scope: &UnitScope,
) -> Result<Vec<DimensionBalance>, JournalError> {
    // Query all lines for the account in the period, we'll parse dimensions in code
    let sql = r"
        SELECT
            jl.dimensions,
            CAST(COALESCE(SUM(jl.base_minor), 0) AS BIGINT) as total_amount
        FROM journal_line jl
        JOIN journal_entry je ON jl.entry_id = je.id
        WHERE je.company_id = ? AND jl.account_number = ? AND je.vat_period = ?
        /*scope*/
        GROUP BY jl.dimensions
    ";

    let statement = scoped_statement(
        scope,
        sql.to_string(),
        vec![
            SeaValue::from(company_id),
            SeaValue::from(account_code),
            SeaValue::from(period),
        ],
    );

    let rows = db.query(&statement).await?;

    let currency = company_currency(db, company_id).await?;

    // Aggregate by dimension value
    let mut dimension_totals: BTreeMap<String, i64> = BTreeMap::new();

    for row in &rows.rows {
        let dimensions_json: String = row
            .get("dimensions")
            .ok_or_else(|| required_column("dimensions"))?;
        let amount: i64 = row
            .get("total_amount")
            .ok_or_else(|| required_column("total_amount"))?;

        // Parse JSON to find the dimension value
        if let Ok(dimensions) = serde_json::from_str::<BTreeMap<String, String>>(&dimensions_json)
            && let Some(dimension_value) = dimensions.get(dimension_key)
        {
            *dimension_totals.entry(dimension_value.clone()).or_insert(0) += amount;
        }
    }

    let mut result = Vec::new();
    for (dimension_value, balance_minor) in dimension_totals {
        result.push(DimensionBalance {
            dimension_value,
            balance: Amount::from_i64(balance_minor, currency),
        });
    }

    Ok(result)
}

/// How many expense accounts a trend report ranks.
pub const TOP_EXPENSE_ACCOUNTS: usize = 8;

/// One month of a trend report. Flows (revenue, expenses, net result) sum the
/// month's entries by VAT period, as the profit and loss report does. Balances
/// are the position at the end of the month, summed over entries posted on or
/// before it, as the balance sheet does.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TrendMonth {
    /// The month, `YYYY-MM`.
    pub period: String,
    /// Income for the month, credit-normal lines negated.
    pub revenue: Amount,
    /// Expenses for the month.
    pub expenses: Amount,
    /// Revenue minus expenses.
    pub net_result: Amount,
    /// Cash and bank accounts (tagged `cash` or `bank`) at month end.
    pub cash: Amount,
    /// Accounts receivable (tagged `ar`) at month end.
    pub receivables: Amount,
    /// Accounts payable (tagged `ap`) at month end, as a positive amount owed.
    pub payables: Amount,
}

/// An expense account ranked by its total in the range.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ExpenseAccountShare {
    /// The account number.
    pub account_code: String,
    /// The account's name key (translated by the client).
    pub name_key: String,
    /// The account's expenses in the range.
    pub amount: Amount,
    /// The share of the range's total expenses, in basis points (hundredths
    /// of a percent), rounded down. Zero when the range has no expenses.
    pub share_bps: i64,
}

/// Revenue and expenses of one dimension value over the range.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DimensionShare {
    /// The dimension value's code.
    pub code: String,
    /// The dimension value's name (the code when it has no name).
    pub name: String,
    /// Income tagged with this value.
    pub revenue: Amount,
    /// Expenses tagged with this value.
    pub expenses: Amount,
}

/// The trend report: one row per month of the range, the top expense
/// accounts, and an optional split by dimension.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Trends {
    /// The first month of the range, `YYYY-MM`.
    pub from: String,
    /// The last month of the range, `YYYY-MM`.
    pub to: String,
    /// One row per month from `from` to `to`, in order, zero-filled.
    pub months: Vec<TrendMonth>,
    /// The expense accounts with the largest totals in the range.
    pub top_expense_accounts: Vec<ExpenseAccountShare>,
    /// The dimension kind the split is by, when one was asked for.
    pub dimension: Option<String>,
    /// The split by dimension value; empty when no dimension was asked for.
    pub by_dimension: Vec<DimensionShare>,
    /// Entries posted per month in the range, by actor (months with none are left out).
    pub activity: Vec<ActivityMonth>,
}

/// The months from `from` to `to` inclusive, as `YYYY-MM`. `None` when either
/// is not a period or `from` is after `to`.
#[must_use]
pub fn month_range(from: &str, to: &str) -> Option<Vec<String>> {
    fn parse(period: &str) -> Option<i32> {
        let bytes = period.as_bytes();
        if bytes.len() != 7 || bytes[4] != b'-' {
            return None;
        }
        let year: i32 = period.get(..4)?.parse().ok()?;
        let month: i32 = period.get(5..)?.parse().ok()?;
        (1..=12).contains(&month).then_some(year * 12 + month - 1)
    }
    let (start, end) = (parse(from)?, parse(to)?);
    if start > end {
        return None;
    }
    Some(
        (start..=end)
            .map(|index| format!("{:04}-{:02}", index / 12, index % 12 + 1))
            .collect(),
    )
}

/// The statements of one trend report: the currency, monthly flows, monthly
/// balances, the top expense accounts and (when asked) the dimension split.
/// The count does not depend on how many months the range spans.
///
/// # Errors
/// Returns [`JournalError::Database`] if a query fails or a stored value is
/// malformed.
pub async fn trends(
    db: &dyn Database,
    company_id: &str,
    from: &str,
    to: &str,
    dimension: Option<&str>,
) -> Result<Trends, JournalError> {
    trends_scoped(
        db,
        company_id,
        from,
        to,
        dimension,
        &UnitScope::Consolidated,
    )
    .await
}

/// [`trends`] for one reporting scope.
///
/// # Errors
/// As [`trends`].
pub async fn trends_scoped(
    db: &dyn Database,
    company_id: &str,
    from: &str,
    to: &str,
    dimension: Option<&str>,
    scope: &UnitScope,
) -> Result<Trends, JournalError> {
    let months = month_range(from, to).unwrap_or_default();
    let currency = company_currency(db, company_id).await?;
    let (Some(first_month), Some(last_month)) = (months.first(), months.last()) else {
        return Ok(Trends {
            from: from.to_owned(),
            to: to.to_owned(),
            months: Vec::new(),
            top_expense_accounts: Vec::new(),
            dimension: dimension.map(str::to_owned),
            by_dimension: Vec::new(),
            activity: Vec::new(),
        });
    };
    let flows = flows_by_month(db, company_id, first_month, last_month, scope).await?;
    let balances = balances_by_month(db, company_id, last_month, scope).await?;
    let total_expenses: i64 = flows.values().map(|(_, expenses)| expenses).sum();

    let month_rows: Vec<TrendMonth> = months
        .iter()
        .map(|month| {
            let (revenue, expenses) = flows.get(month).copied().unwrap_or((0, 0));
            let [cash, receivables, payables] = balances.at(month);
            TrendMonth {
                period: month.clone(),
                revenue: Amount::from_i64(revenue, currency),
                expenses: Amount::from_i64(expenses, currency),
                net_result: Amount::from_i64(revenue - expenses, currency),
                cash: Amount::from_i64(cash, currency),
                receivables: Amount::from_i64(receivables, currency),
                payables: Amount::from_i64(payables, currency),
            }
        })
        .collect();

    let top_expense_accounts = top_expense_accounts(
        db,
        company_id,
        first_month,
        last_month,
        total_expenses,
        currency,
        scope,
    )
    .await?;
    let by_dimension = match dimension {
        Some(kind) => {
            dimension_split(
                db,
                company_id,
                first_month,
                last_month,
                kind,
                currency,
                scope,
            )
            .await?
        }
        None => Vec::new(),
    };
    let activity = activity_by_month(db, company_id, first_month, last_month, scope).await?;

    Ok(Trends {
        from: from.to_owned(),
        to: to.to_owned(),
        months: month_rows,
        top_expense_accounts,
        dimension: dimension.map(str::to_owned),
        by_dimension,
        activity,
    })
}

/// Revenue and expenses per VAT period in the range, as `(income, expense)`
/// P&L values. One statement.
async fn flows_by_month(
    db: &dyn Database,
    company_id: &str,
    first_month: &str,
    last_month: &str,
    scope: &UnitScope,
) -> Result<BTreeMap<String, (i64, i64)>, JournalError> {
    let rows = db
        .query(&scoped_statement(
            scope,
            r"SELECT je.vat_period AS period, a.account_type,
                     CAST(COALESCE(SUM(jl.base_minor), 0) AS BIGINT) AS total
              FROM journal_line jl
              JOIN journal_entry je ON jl.entry_id = je.id
              JOIN account a ON jl.account_number = a.number AND je.company_id = a.company_id
              WHERE je.company_id = ? AND je.vat_period >= ? AND je.vat_period <= ?
                AND a.account_type IN ('income', 'expense')
              /*scope*/
              GROUP BY je.vat_period, a.account_type"
                .to_owned(),
            vec![
                SeaValue::from(company_id),
                SeaValue::from(first_month),
                SeaValue::from(last_month),
            ],
        ))
        .await?;
    let mut flows: BTreeMap<String, (i64, i64)> = BTreeMap::new();
    for row in &rows.rows {
        let period: String = row.get("period").ok_or_else(|| required_column("period"))?;
        let account_type: String = row
            .get("account_type")
            .ok_or_else(|| required_column("account_type"))?;
        let total: i64 = row.get("total").ok_or_else(|| required_column("total"))?;
        let entry = flows.entry(period).or_insert((0, 0));
        match account_type.as_str() {
            // Income is credit-normal: negate to get the P&L value.
            "income" => entry.0 -= total,
            "expense" => entry.1 += total,
            _ => {}
        }
    }
    Ok(flows)
}

/// Running balances of the cash, receivable and payable accounts by month
/// end. One statement covering every posting up to the end of the range.
struct MonthlyBalances {
    cumulative: BTreeMap<String, [i64; 3]>,
}

impl MonthlyBalances {
    /// `[cash, receivables, payables]` at the end of `month`.
    fn at(&self, month: &str) -> [i64; 3] {
        self.cumulative
            .range(..=month.to_owned())
            .next_back()
            .map_or([0; 3], |(_, value)| *value)
    }
}

async fn balances_by_month(
    db: &dyn Database,
    company_id: &str,
    last_month: &str,
    scope: &UnitScope,
) -> Result<MonthlyBalances, JournalError> {
    // The bucket is the posting month, as the balance sheet reads it.
    let rows = db
        .query(&scoped_statement(scope,
            r"SELECT substr(je.posting_date, 1, 7) AS month,
                     CASE WHEN (' ' || a.tags || ' ') LIKE '% cash %' OR (' ' || a.tags || ' ') LIKE '% bank %' THEN 'cash'
                          WHEN (' ' || a.tags || ' ') LIKE '% ar %' THEN 'receivables'
                          ELSE 'payables' END AS bucket,
                     CAST(COALESCE(SUM(jl.base_minor), 0) AS BIGINT) AS total
              FROM journal_line jl
              JOIN journal_entry je ON jl.entry_id = je.id
              JOIN account a ON jl.account_number = a.number AND je.company_id = a.company_id
              WHERE je.company_id = ? AND substr(je.posting_date, 1, 7) <= ?
                AND (
                  (' ' || a.tags || ' ') LIKE '% cash %' OR (' ' || a.tags || ' ') LIKE '% bank %'
                  OR (' ' || a.tags || ' ') LIKE '% ar %' OR (' ' || a.tags || ' ') LIKE '% ap %'
                )
              /*scope*/
              GROUP BY substr(je.posting_date, 1, 7),
                       CASE WHEN (' ' || a.tags || ' ') LIKE '% cash %' OR (' ' || a.tags || ' ') LIKE '% bank %' THEN 'cash'
                            WHEN (' ' || a.tags || ' ') LIKE '% ar %' THEN 'receivables'
                            ELSE 'payables' END"
                .to_owned(),
            vec![SeaValue::from(company_id), SeaValue::from(last_month)],
        ))
        .await?;
    // [cash, receivables, payables] activity per month, payables sign-flipped
    // so every bucket reads as a positive balance.
    let mut activity: BTreeMap<String, [i64; 3]> = BTreeMap::new();
    for row in &rows.rows {
        let month: String = row.get("month").ok_or_else(|| required_column("month"))?;
        let bucket: String = row.get("bucket").ok_or_else(|| required_column("bucket"))?;
        let total: i64 = row.get("total").ok_or_else(|| required_column("total"))?;
        let slot = match bucket.as_str() {
            "cash" => 0,
            "receivables" => 1,
            _ => 2,
        };
        let sign = if slot == 2 { -1 } else { 1 };
        activity.entry(month).or_insert([0; 3])[slot] += sign * total;
    }
    let mut running = [0_i64; 3];
    let mut cumulative: BTreeMap<String, [i64; 3]> = BTreeMap::new();
    for (month, delta) in &activity {
        for (total, change) in running.iter_mut().zip(delta) {
            *total += change;
        }
        cumulative.insert(month.clone(), running);
    }
    Ok(MonthlyBalances { cumulative })
}

/// The expense accounts with the largest totals in the range. One statement.
async fn top_expense_accounts(
    db: &dyn Database,
    company_id: &str,
    first_month: &str,
    last_month: &str,
    total_expenses: i64,
    currency: Currency,
    scope: &UnitScope,
) -> Result<Vec<ExpenseAccountShare>, JournalError> {
    let rows = db
        .query(&scoped_statement(
            scope,
            r"SELECT jl.account_number, a.name_key,
                     CAST(COALESCE(SUM(jl.base_minor), 0) AS BIGINT) AS total
              FROM journal_line jl
              JOIN journal_entry je ON jl.entry_id = je.id
              JOIN account a ON jl.account_number = a.number AND je.company_id = a.company_id
              WHERE je.company_id = ? AND je.vat_period >= ? AND je.vat_period <= ?
                AND a.account_type = 'expense'
              /*scope*/
              GROUP BY jl.account_number, a.name_key
              ORDER BY total DESC, jl.account_number ASC
              LIMIT 8"
                .to_owned(),
            vec![
                SeaValue::from(company_id),
                SeaValue::from(first_month),
                SeaValue::from(last_month),
            ],
        ))
        .await?;
    let mut accounts = Vec::new();
    for row in &rows.rows {
        let account_code: String = row
            .get("account_number")
            .ok_or_else(|| required_column("account_number"))?;
        let name_key: String = row
            .get("name_key")
            .ok_or_else(|| required_column("name_key"))?;
        let total: i64 = row.get("total").ok_or_else(|| required_column("total"))?;
        accounts.push(ExpenseAccountShare {
            account_code,
            name_key,
            amount: Amount::from_i64(total, currency),
            share_bps: share_bps(total, total_expenses),
        });
    }
    Ok(accounts)
}

/// Revenue and expenses per value of one dimension kind over the range. One
/// statement.
async fn dimension_split(
    db: &dyn Database,
    company_id: &str,
    first_month: &str,
    last_month: &str,
    kind: &str,
    currency: Currency,
    scope: &UnitScope,
) -> Result<Vec<DimensionShare>, JournalError> {
    let rows = db
        .query(&scoped_statement(scope,
            r"SELECT jld.value AS code, COALESCE(dv.name, jld.value) AS name, a.account_type,
                     CAST(COALESCE(SUM(jl.base_minor), 0) AS BIGINT) AS total
              FROM journal_line jl
              JOIN journal_entry je ON jl.entry_id = je.id
              JOIN account a ON jl.account_number = a.number AND je.company_id = a.company_id
              JOIN journal_line_dimension jld
                ON jld.company_id = je.company_id AND jld.entry_id = jl.entry_id AND jld.line_no = jl.line_no
              LEFT JOIN dimension_value dv
                ON dv.company_id = jld.company_id AND dv.kind = jld.kind AND dv.code = jld.value
              WHERE je.company_id = ? AND je.vat_period >= ? AND je.vat_period <= ?
                AND jld.kind = ? AND a.account_type IN ('income', 'expense')
              /*scope*/
              GROUP BY jld.value, COALESCE(dv.name, jld.value), a.account_type
              ORDER BY jld.value"
                .to_owned(),
            vec![
                SeaValue::from(company_id),
                SeaValue::from(first_month),
                SeaValue::from(last_month),
                SeaValue::from(kind),
            ],
        ))
        .await?;
    let mut split: BTreeMap<String, DimensionShare> = BTreeMap::new();
    for row in &rows.rows {
        let code: String = row.get("code").ok_or_else(|| required_column("code"))?;
        let name: String = row.get("name").ok_or_else(|| required_column("name"))?;
        let account_type: String = row
            .get("account_type")
            .ok_or_else(|| required_column("account_type"))?;
        let total: i64 = row.get("total").ok_or_else(|| required_column("total"))?;
        let entry = split.entry(code.clone()).or_insert_with(|| DimensionShare {
            code,
            name,
            revenue: Amount::from_i64(0, currency),
            expenses: Amount::from_i64(0, currency),
        });
        match account_type.as_str() {
            "income" => entry.revenue = Amount::from_i64(-total, currency),
            _ => entry.expenses = Amount::from_i64(total, currency),
        }
    }
    Ok(split.into_values().collect())
}

/// `part` as basis points of `whole`, rounded down. Zero when `whole` is not
/// positive. Integer arithmetic only.
#[must_use]
pub fn share_bps(part: i64, whole: i64) -> i64 {
    if whole <= 0 {
        return 0;
    }
    let scaled = i128::from(part) * 10_000 / i128::from(whole);
    i64::try_from(scaled).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod trend_maths {
    use super::{month_range, share_bps};

    #[test]
    fn a_range_lists_every_month_across_a_year_boundary() {
        let months = month_range("2025-11", "2026-02").expect("a range");
        assert_eq!(months, ["2025-11", "2025-12", "2026-01", "2026-02"]);
        assert_eq!(
            month_range("2026-03", "2026-03"),
            Some(vec!["2026-03".to_owned()])
        );
    }

    #[test]
    fn a_reversed_range_or_a_bad_period_has_no_months() {
        assert_eq!(month_range("2026-06", "2026-01"), None);
        assert_eq!(month_range("2026-13", "2026-14"), None);
        assert_eq!(month_range("2026-1", "2026-03"), None);
        assert_eq!(month_range("26-01", "2026-03"), None);
    }

    #[test]
    fn a_share_is_basis_points_rounded_down() {
        assert_eq!(share_bps(1, 3), 3_333);
        assert_eq!(share_bps(20_160_000, 32_260_680), 6_249);
        assert_eq!(share_bps(5, 5), 10_000);
    }

    #[test]
    fn a_share_of_nothing_is_zero() {
        assert_eq!(share_bps(10, 0), 0);
        assert_eq!(share_bps(10, -4), 0);
    }
}

/// Entries posted in one month, split by who posted them.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ActivityMonth {
    /// The month the entries were posted in, `YYYY-MM`.
    pub period: String,
    /// Entries posted by an agent key.
    pub agent: i64,
    /// Entries posted by a person.
    pub human: i64,
}

/// Entries posted per month in the range, by actor. One statement.
async fn activity_by_month(
    db: &dyn Database,
    company_id: &str,
    first_month: &str,
    last_month: &str,
    scope: &UnitScope,
) -> Result<Vec<ActivityMonth>, JournalError> {
    let rows = db
        .query(&scoped_statement(
            scope,
            r"SELECT substr(je.posting_date, 1, 7) AS month, je.actor,
                     CAST(COUNT(*) AS BIGINT) AS total
              FROM journal_entry je
              WHERE je.company_id = ? AND substr(je.posting_date, 1, 7) >= ?
                AND substr(je.posting_date, 1, 7) <= ?
              /*scope*/
              GROUP BY substr(je.posting_date, 1, 7), je.actor"
                .to_owned(),
            vec![
                SeaValue::from(company_id),
                SeaValue::from(first_month),
                SeaValue::from(last_month),
            ],
        ))
        .await?;
    let mut by_month: BTreeMap<String, ActivityMonth> = BTreeMap::new();
    for row in &rows.rows {
        let period: String = row.get("month").ok_or_else(|| required_column("month"))?;
        let actor: String = row.get("actor").ok_or_else(|| required_column("actor"))?;
        let total: i64 = row.get("total").ok_or_else(|| required_column("total"))?;
        let entry = by_month.entry(period.clone()).or_insert(ActivityMonth {
            period,
            agent: 0,
            human: 0,
        });
        match actor.as_str() {
            "human" => entry.human += total,
            _ => entry.agent += total,
        }
    }
    Ok(by_month.into_values().collect())
}

/// One row of the units comparison: a business unit, or the consolidated
/// company, or the eliminations that reconcile the two.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct UnitSummaryRow {
    /// The unit code; `None` for the consolidated and elimination rows.
    pub unit: Option<String>,
    /// Income for the period (credit-normal, shown positive).
    pub revenue: Amount,
    /// Expenses for the period.
    pub expenses: Amount,
    /// Revenue minus expenses.
    pub result: Amount,
    /// The cumulative balance of the unit's cash and bank accounts through
    /// the period (debit positive). Zero when the unit has none.
    pub cash: Amount,
    /// Income from outside the group: the unit's income in entries with no
    /// intercompany line (recharges excluded), as the consolidated view counts
    /// it. The units' external revenues add up to the consolidated revenue,
    /// so a unit's share of the company's revenue divides this by it.
    pub external_revenue: Amount,
}

/// The side-by-side view of every business unit for one period, with the
/// consolidated company total and the intercompany eliminations that take
/// the units to the total.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct UnitsSummary {
    /// The period, `YYYY-MM`.
    pub period: String,
    /// One row per unit, by code. Archived units appear when they have
    /// lines in the period.
    pub units: Vec<UnitSummaryRow>,
    /// Company totals with intercompany entries eliminated.
    pub consolidated: UnitSummaryRow,
    /// The difference between the consolidated total and the sum of the
    /// units: what the intercompany entries add to each unit's books and
    /// take away again in the consolidated view.
    pub eliminations: UnitSummaryRow,
}

/// Compares every business unit for one period. Constant in the number of
/// units: a fixed set of statements, whatever the unit count.
///
/// The unit rows keep their intercompany lines (a unit sees its recharges);
/// the consolidated row drops intercompany entries whole; the eliminations
/// row is the difference, so the rows always add up.
///
/// # Errors
/// [`JournalError::Database`] when a query fails, or a unit's money cannot
/// be represented.
pub async fn units_summary(
    db: &dyn Database,
    company_id: &str,
    period: &str,
) -> Result<UnitsSummary, JournalError> {
    let currency = company_currency(db, company_id).await?;
    let units = crate::units::list_units(db, company_id, true)
        .await
        .map_err(|err| match err {
            AccountsError::Database(db) => JournalError::Database(db),
            other => JournalError::Database(DbError::Query(other.to_string())),
        })?;
    let (per_unit, cash_by_unit) = unit_pnl_and_cash(db, company_id, period).await?;
    let consolidated_cash = consolidated_cash(db, company_id, period).await?;
    let consolidated_pnl =
        profit_and_loss_scoped(db, company_id, period, &UnitScope::Consolidated).await?;

    let external_by_unit = unit_external_revenue(db, company_id, period).await?;
    let row = |unit: Option<String>, revenue: i128, expenses: i128, cash: i128, external: i128| {
        UnitSummaryRow {
            unit,
            revenue: Amount::new(revenue, currency),
            expenses: Amount::new(expenses, currency),
            result: Amount::new(revenue - expenses, currency),
            cash: Amount::new(cash, currency),
            external_revenue: Amount::new(external, currency),
        }
    };
    let mut rows = Vec::new();
    for unit in &units {
        let (revenue, expenses) = per_unit.get(&unit.code).copied().unwrap_or((0, 0));
        let cash = cash_by_unit.get(&unit.code).copied().unwrap_or(0);
        if unit.archived && revenue == 0 && expenses == 0 && cash == 0 {
            continue;
        }
        let external = external_by_unit.get(&unit.code).copied().unwrap_or(0);
        rows.push(row(
            Some(unit.code.clone()),
            revenue,
            expenses,
            cash,
            external,
        ));
    }
    let consolidated = row(
        None,
        consolidated_pnl.revenue.minor(),
        consolidated_pnl.expenses.minor(),
        consolidated_cash,
        consolidated_pnl.revenue.minor(),
    );
    let sum = |field: fn(&UnitSummaryRow) -> i128| -> i128 { rows.iter().map(field).sum() };
    let eliminations = row(
        None,
        consolidated.revenue.minor() - sum(|r| r.revenue.minor()),
        consolidated.expenses.minor() - sum(|r| r.expenses.minor()),
        consolidated.cash.minor() - sum(|r| r.cash.minor()),
        consolidated.external_revenue.minor() - sum(|r| r.external_revenue.minor()),
    );
    Ok(UnitsSummary {
        period: period.to_owned(),
        units: rows,
        consolidated,
        eliminations,
    })
}

/// External income per unit (positive minor units): each unit's income in the
/// entries the consolidated view keeps, so an entry with an intercompany line
/// (a recharge) is left out, exactly as [`UnitScope::Consolidated`] leaves it out.
async fn unit_external_revenue(
    db: &dyn Database,
    company_id: &str,
    period: &str,
) -> Result<BTreeMap<String, i128>, JournalError> {
    let (intercompany_filter, _) = UnitScope::Consolidated.line_filter();
    let sql = format!(
        r"SELECT jld.value AS unit, CAST(COALESCE(SUM(jl.base_minor), 0) AS BIGINT) AS total
              FROM journal_line jl
              JOIN journal_entry je ON jl.entry_id = je.id
              JOIN account a ON jl.account_number = a.number AND je.company_id = a.company_id
              JOIN journal_line_dimension jld
                ON jld.company_id = je.company_id AND jld.entry_id = jl.entry_id
               AND jld.line_no = jl.line_no AND jld.kind = 'unit'
              WHERE je.company_id = ? AND je.vat_period = ? AND a.account_type = 'income'{intercompany_filter}
              GROUP BY jld.value"
    );
    let rows = db
        .query(&Statement::with_values(
            sql,
            vec![SeaValue::from(company_id), SeaValue::from(period)],
        ))
        .await?;
    let mut external = BTreeMap::new();
    for row in &rows.rows {
        let unit: String = row.get("unit").ok_or_else(|| required_column("unit"))?;
        let total: i64 = row.get("total").ok_or_else(|| required_column("total"))?;
        // Income is credit-normal: negate to get the positive revenue.
        external.insert(unit, -i128::from(total));
    }
    Ok(external)
}

/// Income and expense per unit (`(income, expense)` in minor units) and the
/// cumulative cash and bank balance per unit: two statements. Intercompany
/// lines stay in, so a unit sees its own recharges.
async fn unit_pnl_and_cash(
    db: &dyn Database,
    company_id: &str,
    period: &str,
) -> Result<(BTreeMap<String, (i128, i128)>, BTreeMap<String, i128>), JournalError> {
    let pnl_rows = db
        .query(&Statement::with_values(
            r"SELECT jld.value AS unit, a.account_type,
                     CAST(COALESCE(SUM(jl.base_minor), 0) AS BIGINT) AS total
              FROM journal_line jl
              JOIN journal_entry je ON jl.entry_id = je.id
              JOIN account a ON jl.account_number = a.number AND je.company_id = a.company_id
              JOIN journal_line_dimension jld
                ON jld.company_id = je.company_id AND jld.entry_id = jl.entry_id
               AND jld.line_no = jl.line_no AND jld.kind = 'unit'
              WHERE je.company_id = ? AND je.vat_period = ?
                AND a.account_type IN ('income', 'expense')
              GROUP BY jld.value, a.account_type"
                .to_owned(),
            vec![SeaValue::from(company_id), SeaValue::from(period)],
        ))
        .await?;
    let mut per_unit: BTreeMap<String, (i128, i128)> = BTreeMap::new();
    for row in &pnl_rows.rows {
        let unit: String = row.get("unit").ok_or_else(|| required_column("unit"))?;
        let account_type: String = row
            .get("account_type")
            .ok_or_else(|| required_column("account_type"))?;
        let total: i64 = row.get("total").ok_or_else(|| required_column("total"))?;
        let entry = per_unit.entry(unit).or_insert((0, 0));
        match account_type.as_str() {
            "income" => entry.0 -= i128::from(total),
            "expense" => entry.1 += i128::from(total),
            _ => {}
        }
    }
    let cash_rows = db
        .query(&Statement::with_values(
            r"SELECT jld.value AS unit,
                     CAST(COALESCE(SUM(jl.base_minor), 0) AS BIGINT) AS total
              FROM journal_line jl
              JOIN journal_entry je ON jl.entry_id = je.id
              JOIN account a ON jl.account_number = a.number AND je.company_id = a.company_id
              JOIN journal_line_dimension jld
                ON jld.company_id = je.company_id AND jld.entry_id = jl.entry_id
               AND jld.line_no = jl.line_no AND jld.kind = 'unit'
              WHERE je.company_id = ? AND je.vat_period <= ?
                AND ((' ' || a.tags || ' ') LIKE '% cash %' OR (' ' || a.tags || ' ') LIKE '% bank %')
              GROUP BY jld.value"
                .to_owned(),
            vec![SeaValue::from(company_id), SeaValue::from(period)],
        ))
        .await?;
    let mut cash_by_unit: BTreeMap<String, i128> = BTreeMap::new();
    for row in &cash_rows.rows {
        let unit: String = row.get("unit").ok_or_else(|| required_column("unit"))?;
        let total: i64 = row.get("total").ok_or_else(|| required_column("total"))?;
        cash_by_unit.insert(unit, i128::from(total));
    }
    Ok((per_unit, cash_by_unit))
}

/// The company's cash and bank balance through the period, with intercompany
/// entries eliminated (one statement).
async fn consolidated_cash(
    db: &dyn Database,
    company_id: &str,
    period: &str,
) -> Result<i128, JournalError> {
    let rows = db
        .query(&scoped_statement(
            &UnitScope::Consolidated,
            r"SELECT CAST(COALESCE(SUM(jl.base_minor), 0) AS BIGINT) AS total
              FROM journal_line jl
              JOIN journal_entry je ON jl.entry_id = je.id
              JOIN account a ON jl.account_number = a.number AND je.company_id = a.company_id
              WHERE je.company_id = ? AND je.vat_period <= ?
                AND ((' ' || a.tags || ' ') LIKE '% cash %' OR (' ' || a.tags || ' ') LIKE '% bank %')
                /*scope*/",
            vec![SeaValue::from(company_id), SeaValue::from(period)],
        ))
        .await?;
    let total: i64 = match rows.rows.first() {
        Some(row) => row.get("total").ok_or_else(|| required_column("total"))?,
        None => 0,
    };
    Ok(i128::from(total))
}
