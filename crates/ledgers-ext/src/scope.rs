//! Entity scope: which company a principal may access.

use ledgers_commands::{CommandError, Principal};

/// Company id: distinguished from other string ids in the workspace.
#[derive(Clone, PartialEq, Eq, Debug, Hash)]
pub struct CompanyId(pub String);

impl CompanyId {
    /// Creates a company id.
    #[must_use]
    pub fn new(id: String) -> Self {
        CompanyId(id)
    }
}

impl std::fmt::Display for CompanyId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Entity scope: which company a principal may access and isolation rules.
pub trait EntityScope: Send + Sync {
    /// Resolves what company a principal may act on.
    ///
    /// # Errors
    /// Returns an error if the principal is not authorized to access the requested company.
    fn resolve(
        &self,
        principal: &Principal,
        requested: Option<&CompanyId>,
    ) -> Result<CompanyId, CommandError>;
}

/// Open default: single-company isolation.
#[derive(Clone, Debug)]
pub struct SingleCompany {
    pub company: CompanyId,
}

impl SingleCompany {
    /// Creates a scope for the given company.
    #[must_use]
    pub fn new(company: CompanyId) -> Self {
        SingleCompany { company }
    }
}

impl EntityScope for SingleCompany {
    fn resolve(
        &self,
        _principal: &Principal,
        requested: Option<&CompanyId>,
    ) -> Result<CompanyId, CommandError> {
        match requested {
            None => Ok(self.company.clone()),
            Some(req) if req == &self.company => Ok(self.company.clone()),
            Some(_) => Err(CommandError::PolicyDenied),
        }
    }
}

#[cfg(any(test, feature = "testing"))]
pub mod contract_tests {
    use super::{CompanyId, EntityScope};
    use ledgers_commands::Principal;

    /// Contract test for `EntityScope` implementations.
    ///
    /// # Panics
    /// Panics if resolution results do not meet the contract (allow default, allow requested, deny foreign).
    pub fn entity_scope(scope: &dyn EntityScope, company: &CompanyId) {
        let principal = Principal::Person {
            person_id: "test".to_string(),
            role: ledgers_commands::Role::Owner,
        };

        assert!(scope.resolve(&principal, None).is_ok());
        assert!(scope.resolve(&principal, Some(company)).is_ok());

        let foreign = CompanyId::new("other".to_string());
        assert!(scope.resolve(&principal, Some(&foreign)).is_err());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_tests_pass() {
        let company = CompanyId::new("test".to_string());
        let scope = SingleCompany::new(company.clone());
        contract_tests::entity_scope(&scope, &company);
    }
}
