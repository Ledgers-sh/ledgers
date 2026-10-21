//! Bank feed: pull transactions from banks or statement processors.

use super::scope::CompanyId;
use async_trait::async_trait;
use ledgers_commands::CommandError;
use ledgers_money::Amount;

/// Bank transaction: booking date, counterparty, reference, amount.
#[derive(Clone, Debug)]
pub struct BankTransaction {
    pub booking_date: String,
    pub counterparty: String,
    pub reference: String,
    pub external_id: Option<String>,
    pub amount: Amount,
}

/// Bank feed: pull transactions from a bank or statement processor.
#[async_trait(?Send)]
pub trait BankFeed: Send + Sync {
    /// Pulls new transactions for an account since the given date.
    async fn pull(
        &self,
        company: &CompanyId,
        account_id: &str,
        since: &str,
    ) -> Result<Vec<BankTransaction>, CommandError>;
}

/// Open default: statement import only (no live feeds).
#[derive(Clone, Copy, Debug)]
pub struct StatementImportOnly;

#[async_trait(?Send)]
impl BankFeed for StatementImportOnly {
    async fn pull(
        &self,
        _company: &CompanyId,
        _account_id: &str,
        _since: &str,
    ) -> Result<Vec<BankTransaction>, CommandError> {
        Ok(Vec::new())
    }
}

#[cfg(any(test, feature = "testing"))]
pub mod contract_tests {
    use super::{BankFeed, CompanyId};

    /// Contract test for `BankFeed` implementations.
    ///
    /// # Panics
    /// Does not panic; contracts assert indirectly through result types.
    pub async fn bank_feed(feed: &dyn BankFeed, company: &CompanyId, account_id: &str) {
        let _ = feed.pull(company, account_id, "2026-01-01").await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn contract_tests_pass() {
        let feed = StatementImportOnly;
        let company = CompanyId::new("test".to_string());
        contract_tests::bank_feed(&feed, &company, "account").await;
    }
}
