//! Service composition: extension trait implementations wired at startup.

use ledgers_approvals::ApprovalRules;
use ledgers_auth::IdentityProvider;
use ledgers_ext::{
    BankFeed, DocumentIntake, EInvoiceTransport, Filing, UsageMeter,
    scope::{CompanyId, EntityScope},
};
use ledgers_ledger::close::CloseCheckRegistry;
use std::sync::Arc;

/// Service composition: pluggable extension trait implementations.
#[derive(Clone)]
pub struct LedgersConfig {
    pub scope: Arc<dyn EntityScope>,
    pub meter: Arc<dyn UsageMeter>,
    pub intake: Arc<dyn DocumentIntake>,
    pub close_checks: Arc<CloseCheckRegistry>,
    pub bank_feed: Arc<dyn BankFeed>,
    pub einvoice: Arc<dyn EInvoiceTransport>,
    pub filing: Arc<dyn Filing>,
    pub approval_rules: Arc<dyn ApprovalRules>,
    pub identity: Arc<dyn IdentityProvider>,
}

impl LedgersConfig {
    /// Creates a config with all open-source defaults.
    #[must_use]
    pub fn open(company: CompanyId) -> Self {
        use ledgers_approvals::OwnerApprovesAll;
        use ledgers_auth::OwnerPasskeys;
        use ledgers_ext::{
            FileExchange, ManualFiling, SingleCompany, StatementImportOnly, Unmetered,
            UploadAndFolder,
        };

        let mut close_checks = CloseCheckRegistry::new();
        close_checks.register(Box::new(ledgers_ledger::close::DraftEntriesCloseCheck));

        LedgersConfig {
            scope: Arc::new(SingleCompany::new(company)),
            meter: Arc::new(Unmetered),
            intake: Arc::new(UploadAndFolder::upload_only()),
            close_checks: Arc::new(close_checks),
            bank_feed: Arc::new(StatementImportOnly),
            einvoice: Arc::new(FileExchange),
            filing: Arc::new(ManualFiling),
            approval_rules: Arc::new(OwnerApprovesAll),
            identity: Arc::new(OwnerPasskeys),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_creates_defaults() {
        let company = CompanyId::new("test-co".to_string());
        let config = LedgersConfig::open(company);
        let _ = (&config.scope, &config.meter, &config.identity);
    }
}
