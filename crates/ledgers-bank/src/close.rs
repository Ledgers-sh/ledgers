//! Close check for bank reconciliation (issue #48).
//!
//! A period blocks close if any active bank account has unmatched lines
//! dated within the period, or if statement and ledger balances do not
//! agree at the period end (the reconciliation residual is non-zero).

use cratefield_core::{Database, DbError, Statement};
use ledgers_ledger::close::{Blocker, CloseCheck, CloseCheckContext};
use ledgers_ledger::error::AccountsError;
use ledgers_ledger::journal_port::BoxFuture;
use sea_query::Value;

/// Compute the sums and counts of an account's lines up to a date, by status:
/// `(unmatched total, unmatched count, ignored total, ignored count)`.
/// This is extracted from commands.rs so both the reconcile command
/// and the close check can reuse the same logic.
pub(crate) async fn balance_summary(
    db: &dyn Database,
    company_id: &str,
    account_id: &str,
    as_of: &str,
) -> Result<(i64, u32, i64, u32), DbError> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT status, COUNT(*) AS n, COALESCE(SUM(minor), 0) AS total FROM bank_transaction \
             WHERE company_id = ? AND account_id = ? AND booking_date <= ? GROUP BY status"
                .to_owned(),
            vec![
                Value::from(company_id),
                Value::from(account_id),
                Value::from(as_of),
            ],
        ))
        .await?;
    let (mut unmatched, mut unmatched_n, mut ignored, mut ignored_n) = (0i64, 0u32, 0i64, 0u32);
    for row in &rows.rows {
        let status = row.get::<String>("status").unwrap_or_default();
        let n = u32::try_from(row.get::<i64>("n").unwrap_or(0)).unwrap_or(u32::MAX);
        let total = row.get::<i64>("total").unwrap_or(0);
        match status.as_str() {
            "unmatched" => {
                unmatched = total;
                unmatched_n = n;
            }
            "ignored" => {
                ignored = total;
                ignored_n = n;
            }
            _ => {}
        }
    }
    Ok((unmatched, unmatched_n, ignored, ignored_n))
}

/// The close check for bank reconciliation.
/// Reports blockers when:
/// - An account has unmatched lines dated within the period being closed.
/// - An account's statement and ledger balances do not agree at period end
///   (the residual is non-zero).
pub struct BankReconciliationCloseCheck;

impl CloseCheck for BankReconciliationCloseCheck {
    fn id(&self) -> &'static str {
        "bank.reconciliation"
    }

    fn blockers<'a>(
        &'a self,
        ctx: CloseCheckContext<'a>,
    ) -> BoxFuture<'a, Result<Vec<Blocker>, AccountsError>> {
        Box::pin(async move {
            let mut blockers = Vec::new();

            // Get all active bank accounts.
            let accounts_stmt = Statement::with_values(
                "SELECT id, currency FROM bank_account \
                 WHERE company_id = ? AND archived_at IS NULL \
                 ORDER BY id"
                    .to_owned(),
                vec![Value::from(ctx.company_id)],
            );

            let accounts = ctx.db.query(&accounts_stmt).await?;

            for account in &accounts.rows {
                let account_id = account.get::<String>("id").ok_or_else(|| {
                    AccountsError::Database(DbError::Query("missing account id".to_string()))
                })?;
                let _currency = account.get::<String>("currency").ok_or_else(|| {
                    AccountsError::Database(DbError::Query("missing currency".to_string()))
                })?;

                // Get the account row to access opening_minor and latest statement.
                let account_detail_stmt = Statement::with_values(
                    "SELECT opening_minor FROM bank_account WHERE company_id = ? AND id = ?"
                        .to_owned(),
                    vec![Value::from(ctx.company_id), Value::from(&account_id)],
                );
                let account_detail_rows = ctx.db.query(&account_detail_stmt).await?;
                let account_row = account_detail_rows.first().ok_or_else(|| {
                    AccountsError::Database(DbError::Query("account row not found".to_string()))
                })?;
                let opening_minor = account_row.get::<i64>("opening_minor").unwrap_or(0);

                // Get the latest statement's closing balance at or before the period end.
                let statement_stmt = Statement::with_values(
                    "SELECT closing_minor FROM bank_statement \
                     WHERE company_id = ? AND account_id = ? AND closing_date <= ? \
                     ORDER BY closing_date DESC LIMIT 1"
                        .to_owned(),
                    vec![
                        Value::from(ctx.company_id),
                        Value::from(&account_id),
                        Value::from(ctx.ends_on),
                    ],
                );
                let statement_rows = ctx.db.query(&statement_stmt).await?;
                let statement_minor = statement_rows
                    .first()
                    .and_then(|r| r.get::<i64>("closing_minor"))
                    .unwrap_or(opening_minor);

                // Get the ledger balance for the account at the period end.
                let ledger_balance_stmt = Statement::with_values(
                    "SELECT COALESCE(SUM(base_minor), 0) as balance FROM journal_line \
                     WHERE account_number = (SELECT ledger_account_number FROM bank_account WHERE id = ?) \
                     AND entry_id IN (SELECT id FROM journal_entry WHERE company_id = ? AND posting_date <= ?)"
                        .to_owned(),
                    vec![
                        Value::from(&account_id),
                        Value::from(ctx.company_id),
                        Value::from(ctx.ends_on),
                    ],
                );
                let ledger_rows = ctx.db.query(&ledger_balance_stmt).await?;
                let ledger_minor = ledger_rows
                    .first()
                    .and_then(|r| r.get::<i64>("balance"))
                    .unwrap_or(0);

                // Get all line totals for residual calculation.
                let (unmatched_total, unmatched_n, ignored_total, _) =
                    balance_summary(ctx.db, ctx.company_id, &account_id, ctx.ends_on).await?;

                // Compute the residual (unexplained difference).
                let difference = statement_minor - ledger_minor;
                let residual = difference - unmatched_total - ignored_total;

                // Check for unmatched lines within the period.
                if unmatched_n > 0 {
                    blockers.push(Blocker {
                        check_id: self.id().to_owned(),
                        kind: "unmatched-lines".to_owned(),
                        count: i64::from(unmatched_n),
                        link: format!(
                            "/companies/{}/bank/accounts/{}/transactions?status=unmatched",
                            ctx.company_id, account_id
                        ),
                    });
                }

                // Check for residual (statement != ledger balance).
                if residual != 0 {
                    blockers.push(Blocker {
                        check_id: self.id().to_owned(),
                        kind: "unreconciled-balance".to_owned(),
                        count: 1,
                        link: format!(
                            "/companies/{}/bank/accounts/{}/reconcile",
                            ctx.company_id, account_id
                        ),
                    });
                }
            }

            Ok(blockers)
        })
    }
}
