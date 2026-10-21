//! Tax return filing: submit tax returns to authorities.

use super::scope::CompanyId;
use async_trait::async_trait;
use ledgers_commands::CommandError;

/// Tax return: kind, period, filename, content.
#[derive(Clone, Debug)]
pub struct TaxReturn {
    pub kind: String,
    pub period: String,
    pub file_name: String,
    pub bytes: Vec<u8>,
}

/// Outcome of filing a tax return.
#[derive(Clone, Debug)]
pub enum FilingReceipt {
    ManualFiling(TaxReturn),
    Submitted { reference: String },
}

/// Filing: submit tax returns to authorities.
#[async_trait(?Send)]
pub trait Filing: Send + Sync {
    /// Submits a tax return.
    async fn submit(
        &self,
        company: &CompanyId,
        return_: &TaxReturn,
    ) -> Result<FilingReceipt, CommandError>;
}

/// Open default: manual submission (file export).
#[derive(Clone, Copy, Debug)]
pub struct ManualFiling;

#[async_trait(?Send)]
impl Filing for ManualFiling {
    async fn submit(
        &self,
        _company: &CompanyId,
        return_: &TaxReturn,
    ) -> Result<FilingReceipt, CommandError> {
        Ok(FilingReceipt::ManualFiling(return_.clone()))
    }
}

#[cfg(any(test, feature = "testing"))]
pub mod contract_tests {
    use super::{CompanyId, Filing, FilingReceipt, TaxReturn};

    /// Contract test for `Filing` implementations.
    ///
    /// # Panics
    /// Panics if a `FilingReceipt::Submitted` result has an empty reference.
    pub async fn filing(filing: &dyn Filing, company: &CompanyId) {
        let return_ = TaxReturn {
            kind: "vat".to_string(),
            period: "2026-Q1".to_string(),
            file_name: "return.xml".to_string(),
            bytes: b"test".to_vec(),
        };

        if let Ok(FilingReceipt::Submitted { reference }) = filing.submit(company, &return_).await {
            assert!(!reference.is_empty());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn contract_tests_pass() {
        let filing = ManualFiling;
        let company = CompanyId::new("test".to_string());
        contract_tests::filing(&filing, &company).await;
    }
}
