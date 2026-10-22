//! Reporting module for trial balance, profit & loss, balance sheet,
//! and dimension-based analysis.
//!
//! This module provides functions to generate key financial reports by querying
//! the journal entries and aggregating balances by account, period, and dimension.
//! All queries operate over immutable posted entries (docs/PLAN.md §3 rule 3).

use cratefield_core::{Database, DbError, Statement};
use ledgers_money::{Amount, Currency};
use sea_query::Value as SeaValue;
use std::str::FromStr;

use crate::entry::JournalError;

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
async fn company_currency(db: &dyn Database, company_id: &str) -> Result<Currency, JournalError> {
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
    let sql = r"
        SELECT
            jl.account_number,
            je.base_currency,
            CAST(COALESCE(SUM(CASE WHEN jl.base_minor > 0 THEN jl.base_minor ELSE 0 END), 0) AS BIGINT) as total_debits,
            CAST(COALESCE(SUM(CASE WHEN jl.base_minor < 0 THEN -jl.base_minor ELSE 0 END), 0) AS BIGINT) as total_credits
        FROM journal_line jl
        JOIN journal_entry je ON jl.entry_id = je.id
        WHERE je.company_id = ? AND je.vat_period = ?
        GROUP BY jl.account_number, je.base_currency
        ORDER BY jl.account_number
    ";

    let statement = Statement::with_values(
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
    let sql = r"
        SELECT
            a.account_type,
            COALESCE(SUM(jl.base_minor), 0) as total_amount
        FROM journal_line jl
        JOIN journal_entry je ON jl.entry_id = je.id
        JOIN account a ON jl.account_number = a.number AND je.company_id = a.company_id
        WHERE je.company_id = ? AND je.vat_period = ? AND a.account_type IN ('income', 'expense')
        GROUP BY a.account_type
    ";

    let statement = Statement::with_values(
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
    let sql = r"
        SELECT
            a.account_type,
            CAST(COALESCE(SUM(jl.base_minor), 0) AS BIGINT) as total_amount
        FROM journal_line jl
        JOIN journal_entry je ON jl.entry_id = je.id
        JOIN account a ON jl.account_number = a.number AND je.company_id = a.company_id
        WHERE je.company_id = ? AND je.posting_date <= ? AND a.account_type IN ('asset', 'liability', 'equity')
        GROUP BY a.account_type
    ";

    let statement = Statement::with_values(
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
    ";
    let pnl_rows = db
        .query(&Statement::with_values(
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
    // Query all lines for the account in the period, we'll parse dimensions in code
    let sql = r"
        SELECT
            jl.dimensions,
            COALESCE(SUM(jl.base_minor), 0) as total_amount
        FROM journal_line jl
        JOIN journal_entry je ON jl.entry_id = je.id
        WHERE je.company_id = ? AND jl.account_number = ? AND je.vat_period = ?
        GROUP BY jl.dimensions
    ";

    let statement = Statement::with_values(
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
    let mut dimension_totals: std::collections::BTreeMap<String, i64> =
        std::collections::BTreeMap::new();

    for row in &rows.rows {
        let dimensions_json: String = row
            .get("dimensions")
            .ok_or_else(|| required_column("dimensions"))?;
        let amount: i64 = row
            .get("total_amount")
            .ok_or_else(|| required_column("total_amount"))?;

        // Parse JSON to find the dimension value
        if let Ok(dimensions) =
            serde_json::from_str::<std::collections::BTreeMap<String, String>>(&dimensions_json)
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
