//! Close check for subledger reconciliation (issue #47).
//!
//! A period blocks close if the subledger reconciliation has non-zero differences
//! for AR or AP control accounts as of the period's end date.

use ledgers_ledger::close::{Blocker, CloseCheck, CloseCheckContext};
use ledgers_ledger::error::AccountsError;
use ledgers_ledger::journal_port::BoxFuture;

use crate::reconcile::reconcile;
use cratefield_core::Statement;
use sea_query::Value;

/// Close check for subledger reconciliation mismatches.
pub struct SubledgerReconciliationCheck;

impl CloseCheck for SubledgerReconciliationCheck {
    fn id(&self) -> &'static str {
        "subledger.reconciliation"
    }

    fn blockers<'a>(
        &'a self,
        ctx: CloseCheckContext<'a>,
    ) -> BoxFuture<'a, Result<Vec<Blocker>, AccountsError>> {
        Box::pin(async move {
            let recon = reconcile(ctx.db, ctx.company_id, ctx.ends_on).await?;

            let mut blockers = Vec::new();
            for ledger_recon in recon {
                if ledger_recon.difference != 0 {
                    blockers.push(Blocker {
                        check_id: self.id().to_owned(),
                        kind: "subledger-mismatch".to_owned(),
                        count: 1,
                        link: format!(
                            "/companies/{}/reconciliation?ledger={}&date={}",
                            ctx.company_id, ledger_recon.ledger, ctx.ends_on
                        ),
                    });
                }
            }

            // Each open item's remaining balance must equal its original amount
            // less every allocation against it, in both currencies.
            let rows = ctx
                .db
                .query(&Statement::with_values(
                    "SELECT oi.id, oi.tx_original_minor, oi.tx_remaining_minor, \
                            oi.base_original_minor, oi.base_remaining_minor, \
                            CAST(COALESCE((SELECT SUM(a.tx_minor) FROM allocation a \
                                           WHERE a.open_item_id = oi.id), 0) AS BIGINT) AS alloc_tx, \
                            CAST(COALESCE((SELECT SUM(a.base_minor) FROM allocation a \
                                           WHERE a.open_item_id = oi.id), 0) AS BIGINT) AS alloc_base \
                     FROM open_item oi WHERE oi.company_id = ?"
                        .to_owned(),
                    vec![Value::from(ctx.company_id)],
                ))
                .await?;
            for row in &rows.rows {
                let id: String = row.get("id").unwrap_or_default();
                let tx_original: i64 = row.get("tx_original_minor").unwrap_or(0);
                let tx_remaining: i64 = row.get("tx_remaining_minor").unwrap_or(0);
                let base_original: i64 = row.get("base_original_minor").unwrap_or(0);
                let base_remaining: i64 = row.get("base_remaining_minor").unwrap_or(0);
                let alloc_tx: i64 = row.get("alloc_tx").unwrap_or(0);
                let alloc_base: i64 = row.get("alloc_base").unwrap_or(0);
                if tx_remaining != tx_original - alloc_tx
                    || base_remaining != base_original - alloc_base
                {
                    blockers.push(Blocker {
                        check_id: self.id().to_owned(),
                        kind: "open-item-mismatch".to_owned(),
                        count: 1,
                        link: format!("/companies/{}/reconciliation?item={}", ctx.company_id, id),
                    });
                }
            }

            Ok(blockers)
        })
    }
}
