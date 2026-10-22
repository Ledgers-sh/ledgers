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
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BalanceSheet {
    /// Total assets (sum of asset account balances).
    pub assets: Amount,
    /// Total liabilities (sum of liability account balances).
    pub liabilities: Amount,
    /// Total equity (sum of equity account balances).
    pub equity: Amount,
}

/// One dimension value's balance for an account.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DimensionBalance {
    /// The dimension value (e.g., cost centre code or customer id).
    pub dimension_value: String,
    /// Balance for this dimension (debit-positive signed amount).
    pub balance: Amount,
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
            COALESCE(SUM(CASE WHEN jl.base_minor > 0 THEN jl.base_minor ELSE 0 END), 0) as total_debits,
            COALESCE(SUM(CASE WHEN jl.base_minor < 0 THEN -jl.base_minor ELSE 0 END), 0) as total_credits
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

    // Query base currency from the period
    let currency_sql = r"
        SELECT DISTINCT je.base_currency
        FROM journal_entry je
        WHERE je.company_id = ? AND je.vat_period = ?
        LIMIT 1
    ";
    let currency_statement = Statement::with_values(
        currency_sql.to_string(),
        vec![SeaValue::from(company_id), SeaValue::from(period)],
    );
    let currency_rows = db.query(&currency_statement).await?;
    let currency = if let Some(row) = currency_rows.rows.first() {
        let code: String = row
            .get("base_currency")
            .ok_or_else(|| required_column("base_currency"))?;
        Currency::from_str(&code)?
    } else {
        // Default to EUR if no entries found
        Currency::from_str("EUR")?
    };

    let revenue = Amount::from_i64(revenue_minor, currency);
    let expenses = Amount::from_i64(expenses_minor, currency);
    let net_income = Amount::from_i64(revenue_minor - expenses_minor, currency);

    Ok(ProfitAndLoss {
        revenue,
        expenses,
        net_income,
    })
}

/// Generates a balance sheet as of the given date: cumulative balances
/// from all entries up to and including that date.
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
/// A `BalanceSheet` struct with assets, liabilities, and equity totals.
pub async fn balance_sheet(
    db: &dyn Database,
    company_id: &str,
    as_of_date: &str,
) -> Result<BalanceSheet, JournalError> {
    let sql = r"
        SELECT
            a.account_type,
            COALESCE(SUM(jl.base_minor), 0) as total_amount
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

    // Query base currency from the company
    let currency_sql = r"
        SELECT DISTINCT je.base_currency
        FROM journal_entry je
        WHERE je.company_id = ? AND je.posting_date <= ?
        LIMIT 1
    ";
    let currency_statement = Statement::with_values(
        currency_sql.to_string(),
        vec![SeaValue::from(company_id), SeaValue::from(as_of_date)],
    );
    let currency_rows = db.query(&currency_statement).await?;
    let currency = if let Some(row) = currency_rows.rows.first() {
        let code: String = row
            .get("base_currency")
            .ok_or_else(|| required_column("base_currency"))?;
        Currency::from_str(&code)?
    } else {
        // Default to EUR if no entries found
        Currency::from_str("EUR")?
    };

    let assets = Amount::from_i64(assets_minor, currency);
    let liabilities = Amount::from_i64(liabilities_minor, currency);
    let equity = Amount::from_i64(equity_minor, currency);

    Ok(BalanceSheet {
        assets,
        liabilities,
        equity,
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

    // Query base currency
    let currency_sql = r"
        SELECT DISTINCT je.base_currency
        FROM journal_entry je
        WHERE je.company_id = ? AND je.vat_period = ?
        LIMIT 1
    ";
    let currency_statement = Statement::with_values(
        currency_sql.to_string(),
        vec![SeaValue::from(company_id), SeaValue::from(period)],
    );
    let currency_rows = db.query(&currency_statement).await?;
    let currency = if let Some(row) = currency_rows.rows.first() {
        let code: String = row
            .get("base_currency")
            .ok_or_else(|| required_column("base_currency"))?;
        Currency::from_str(&code)?
    } else {
        // Default to EUR if no entries found
        Currency::from_str("EUR")?
    };

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
