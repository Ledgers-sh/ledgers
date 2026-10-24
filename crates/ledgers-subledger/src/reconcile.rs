//! Subledger reconciliation: comparing control account GL balances with open items.
//!
//! Reconciles AR and AP balances as of a date, comparing:
//! - Subledger balance: Σ `open_item.base_original_minor` − Σ `allocation.base_minor` (as of date)
//! - GL balance: Σ `journal_line.base_minor` for control account (as of date), signed appropriately
//!   (AR: debit-positive; AP: credit-positive, so negated)

use cratefield_core::{Database, Statement};
use ledgers_ledger::error::AccountsError;
use schemars::JsonSchema;
use sea_query::Value;
use serde::{Deserialize, Serialize};

/// One ledger's reconciliation: control account number, subledger balance, GL balance, and difference.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct LedgerReconciliation {
    /// The ledger: "ar" or "ap".
    pub ledger: String,
    /// The control account number (tagged ar/ap).
    pub control_account: String,
    /// Subledger balance in base currency minor units, signed in control account's normal direction.
    pub subledger_balance: i64,
    /// GL balance in base currency minor units, signed in control account's normal direction.
    pub gl_balance: i64,
    /// Difference: GL - subledger. Should be zero when in balance.
    pub difference: i64,
}

/// Reconciles AR and AP subledger balances as of a date.
///
/// For each ledger (AR, AP), computes:
/// - Subledger: Σ `open_item.base_original_minor` − Σ `allocation.base_minor` (for items posted ≤ `as_of` with allocations ≤ `as_of`)
/// - GL: Σ `journal_line.base_minor` for control account (for lines posted ≤ `as_of`), signed in control direction
///   - AR: debit-positive (no adjustment needed)
///   - AP: credit-positive (negate the sum)
/// - Difference: GL − subledger
///
/// # Errors
/// [`AccountsError::Database`] when a query fails.
#[allow(clippy::similar_names)]
pub async fn reconcile(
    db: &dyn Database,
    company_id: &str,
    as_of: &str,
) -> Result<Vec<LedgerReconciliation>, AccountsError> {
    // Find AR and AP control accounts
    let accounts = load_control_accounts(db, company_id).await?;
    let ar_account = &accounts.0;
    let ap_account = &accounts.1;

    let mut results = Vec::new();

    // Reconcile AR
    if let Some(ar_num) = ar_account {
        let subledger_bal = subledger_balance(db, company_id, "ar", as_of).await?;
        let gl_bal = gl_balance(db, company_id, ar_num, as_of, false).await?; // AR: no negation
        results.push(LedgerReconciliation {
            ledger: "ar".to_string(),
            control_account: ar_num.clone(),
            subledger_balance: subledger_bal,
            gl_balance: gl_bal,
            difference: gl_bal - subledger_bal,
        });
    }

    // Reconcile AP
    if let Some(ap_num) = ap_account {
        let subledger_bal = subledger_balance(db, company_id, "ap", as_of).await?;
        let gl_bal = gl_balance(db, company_id, ap_num, as_of, true).await?; // AP: negate for credit-positive
        results.push(LedgerReconciliation {
            ledger: "ap".to_string(),
            control_account: ap_num.clone(),
            subledger_balance: subledger_bal,
            gl_balance: gl_bal,
            difference: gl_bal - subledger_bal,
        });
    }

    Ok(results)
}

/// Loads the AR and AP control account numbers from the chart.
async fn load_control_accounts(
    db: &dyn Database,
    company_id: &str,
) -> Result<(Option<String>, Option<String>), AccountsError> {
    // Use ledgers-ledger's account listing to get proper Account entries with tags
    let entries = ledgers_ledger::commands::list_accounts(db, company_id, true)
        .await
        .map_err(|e| AccountsError::Database(cratefield_core::DbError::Query(e.to_string())))?;

    let mut ar = None;
    let mut ap = None;

    for entry in entries {
        if entry.account.tags.iter().any(|t| t == "ar") && ar.is_none() {
            ar = Some(entry.account.number.clone());
        }
        if entry.account.tags.iter().any(|t| t == "ap") && ap.is_none() {
            ap = Some(entry.account.number);
        }
    }

    Ok((ar, ap))
}

/// Computes subledger balance for a ledger as of a date.
///
/// Formula: Σ `open_item.base_original_minor` − Σ `allocation.base_minor`
/// where `open_item.posting_date` ≤ `as_of` and `allocation.date` ≤ `as_of`.
async fn subledger_balance(
    db: &dyn Database,
    company_id: &str,
    ledger: &str,
    as_of: &str,
) -> Result<i64, AccountsError> {
    // Sum of original amounts for open items posted on or before as_of
    let original_rows = db
        .query(&Statement::with_values(
            "SELECT CAST(COALESCE(SUM(base_original_minor), 0) AS BIGINT) AS total \
             FROM open_item \
             WHERE company_id = ? AND ledger = ? AND posting_date <= ?"
                .to_owned(),
            vec![
                Value::from(company_id),
                Value::from(ledger),
                Value::from(as_of),
            ],
        ))
        .await?;

    let original_total: i64 = original_rows
        .first()
        .and_then(|row| row.get::<i64>("total"))
        .unwrap_or(0);

    // Sum of allocation amounts for items in this ledger with allocations on or before as_of
    let allocation_rows = db
        .query(&Statement::with_values(
            "SELECT CAST(COALESCE(SUM(a.base_minor), 0) AS BIGINT) AS total \
             FROM allocation a \
             JOIN open_item oi ON a.open_item_id = oi.id \
             WHERE oi.company_id = ? AND oi.ledger = ? AND a.date <= ?"
                .to_owned(),
            vec![
                Value::from(company_id),
                Value::from(ledger),
                Value::from(as_of),
            ],
        ))
        .await?;

    let allocation_total: i64 = allocation_rows
        .first()
        .and_then(|row| row.get::<i64>("total"))
        .unwrap_or(0);

    Ok(original_total - allocation_total)
}

/// Computes GL balance for a control account as of a date.
///
/// Sums `journal_line.base_minor` for the control account in posted entries with `posting_date` ≤ `as_of`.
/// If `negate` is true (for AP), negates the result to express as credit-positive.
async fn gl_balance(
    db: &dyn Database,
    company_id: &str,
    account_number: &str,
    as_of: &str,
    negate: bool,
) -> Result<i64, AccountsError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT CAST(COALESCE(SUM(jl.base_minor), 0) AS BIGINT) AS total \
             FROM journal_line jl \
             JOIN journal_entry je ON jl.entry_id = je.id \
             WHERE je.company_id = ? AND jl.account_number = ? AND je.posting_date <= ? \
               AND je.status = 'posted'"
                .to_owned(),
            vec![
                Value::from(company_id),
                Value::from(account_number),
                Value::from(as_of),
            ],
        ))
        .await?;

    let total: i64 = rows
        .first()
        .and_then(|row| row.get::<i64>("total"))
        .unwrap_or(0);

    Ok(if negate { -total } else { total })
}

#[cfg(test)]
mod tests {
    #[test]
    fn negate_flag_reverses_balance() {
        // GL balance of 1000 without negate = 1000 (debit-positive)
        // GL balance of 1000 with negate = -1000 (credit-positive)
        assert_eq!(gl_balance_compute(1000, false), 1000);
        assert_eq!(gl_balance_compute(1000, true), -1000);
        assert_eq!(gl_balance_compute(-500, false), -500);
        assert_eq!(gl_balance_compute(-500, true), 500);
    }

    fn gl_balance_compute(balance: i64, negate: bool) -> i64 {
        if negate { -balance } else { balance }
    }
}
