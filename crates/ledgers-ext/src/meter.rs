//! Usage meter: count actions and decide admission.

use super::scope::CompanyId;
use async_trait::async_trait;
use ledgers_commands::CommandError;

/// Metered actions.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[non_exhaustive]
pub enum MeteredAction {
    DocumentProcessed,
}

/// Admission decision: `Allow` or `Pause(reason)`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Admission {
    Allow,
    Pause(String),
}

/// Usage meter: track actions and decide admission.
#[async_trait(?Send)]
pub trait UsageMeter: Send + Sync {
    /// Checks whether an action is permitted.
    async fn admit(&self, company: &CompanyId, action: MeteredAction) -> Admission;

    /// Records that an action was completed.
    async fn record(
        &self,
        company: &CompanyId,
        action: MeteredAction,
        count: u64,
    ) -> Result<(), CommandError>;
}

/// Open default: all actions always admitted.
#[derive(Clone, Copy, Debug)]
pub struct Unmetered;

#[async_trait(?Send)]
impl UsageMeter for Unmetered {
    async fn admit(&self, _company: &CompanyId, _action: MeteredAction) -> Admission {
        Admission::Allow
    }

    async fn record(
        &self,
        _company: &CompanyId,
        _action: MeteredAction,
        _count: u64,
    ) -> Result<(), CommandError> {
        Ok(())
    }
}

#[cfg(any(test, feature = "testing"))]
pub mod contract_tests {
    use super::{Admission, CompanyId, MeteredAction, UsageMeter};

    /// Contract test for `UsageMeter` implementations.
    ///
    /// # Panics
    /// Panics if a pause reason is empty.
    pub async fn usage_meter(meter: &dyn UsageMeter, company: &CompanyId) {
        let action = MeteredAction::DocumentProcessed;
        let admission = meter.admit(company, action).await;
        if let Admission::Pause(reason) = admission {
            assert!(!reason.is_empty());
        }
        let _ = meter.record(company, action, 1).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn contract_tests_pass() {
        let meter = Unmetered;
        let company = CompanyId::new("test".to_string());
        contract_tests::usage_meter(&meter, &company).await;
    }
}
