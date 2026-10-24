//! Close check for subledger reconciliation (issue #47).
//!
//! A period blocks close if the subledger reconciliation has non-zero differences
//! for AR or AP control accounts as of the period's end date.

use ledgers_ledger::close::{Blocker, CloseCheck, CloseCheckContext};
use ledgers_ledger::error::AccountsError;
use ledgers_ledger::journal_port::BoxFuture;

use crate::reconcile::reconcile;

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

            Ok(blockers)
        })
    }
}
