//! Authentication, tiers, principals and limits.
//!
//! A [`Principal`] is either the company owner (a [`Person`]) or an agent
//! key with a [`Tier`] and [`Limits`] per currency. The [`Authenticator`]
//! trait maps a credential to a principal or an error.

use ledgers_money::Currency;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::ops::Deref;

/// The policy tier an agent key carries.
///
/// Tiers are ordered — `Tier::Read < Tier::Draft < Tier::PostWithinLimits`.
/// A command that requires `Draft` denies an agent with `Read` only.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tier {
    /// Read-only: query commands, no mutations.
    Read,
    /// Draft entries, invoices, bills, etc.; no posting.
    Draft,
    /// Post within per-currency limits; denied above.
    #[serde(rename = "post-within-limits")]
    PostWithinLimits,
}

impl Tier {
    /// The string form used in ledgers-audit Actor and serialization.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Read => "read",
            Tier::Draft => "draft",
            Tier::PostWithinLimits => "post-within-limits",
        }
    }
}

impl std::fmt::Display for Tier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Tier {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "read" => Ok(Tier::Read),
            "draft" => Ok(Tier::Draft),
            "post-within-limits" => Ok(Tier::PostWithinLimits),
            _ => Err(format!("unknown tier: {s}")),
        }
    }
}

/// Per-currency limit for an agent key: base amount and transaction amount
/// ceilings in minor units (i128).
///
/// A limit of 0 for a currency means no transactions in that currency are
/// allowed for the agent.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Limit {
    /// Maximum single base amount in minor units; 0 = no transactions allowed.
    pub base_ceiling: i128,
    /// Maximum single transaction amount in minor units; 0 = no transactions allowed.
    pub transaction_ceiling: i128,
}

/// Per-currency ceilings for an agent key: what the policy engine checks
/// against the preview.
///
/// A currency with no entry means that currency is disallowed (ceiling 0).
/// Keys are ISO 4217 currency codes (e.g., "EUR", "USD").
///
/// Invalid currency codes are rejected at construction and on deserialization.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Limits {
    inner: BTreeMap<String, Limit>,
}

impl Limits {
    /// Creates an empty limits map.
    #[must_use]
    pub const fn new() -> Self {
        Limits {
            inner: BTreeMap::new(),
        }
    }

    /// Adds a per-currency limit, validating the currency code.
    ///
    /// # Errors
    /// Returns an error if the currency code is invalid.
    pub fn insert(mut self, code: &str, limit: Limit) -> Result<Self, String> {
        Currency::from_code(code)
            .map_err(|_| format!("invalid currency code in limits: {code}"))?;
        self.inner.insert(code.to_string(), limit);
        Ok(self)
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::new()
    }
}

impl Deref for Limits {
    type Target = BTreeMap<String, Limit>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl Serialize for Limits {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.inner.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Limits {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let inner = BTreeMap::<String, Limit>::deserialize(deserializer)?;
        for code in inner.keys() {
            Currency::from_code(code).map_err(|_| {
                serde::de::Error::custom(format!("invalid currency code in limits: {code}"))
            })?;
        }
        Ok(Limits { inner })
    }
}

/// A role a person holds in the company (docs/permissions.md). The role is
/// the person's membership role, read from the organization at sign-in and on
/// every command; the permission matrix lives in `ledgers-policy`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    /// Everything, including ownership transfer and removing members.
    Owner,
    /// Everything an owner does except transferring ownership.
    Admin,
    /// Posts, reverses, bills, accounts; no settings, keys or people.
    Accountant,
    /// Decides approval requests; reads the books.
    Approver,
    /// Reads the books.
    Viewer,
    /// Reads the books, the audit log and exports.
    Auditor,
}

impl Role {
    /// Every role, in the order the matrix and the app list them.
    pub const ALL: [Role; 6] = [
        Role::Owner,
        Role::Admin,
        Role::Accountant,
        Role::Approver,
        Role::Viewer,
        Role::Auditor,
    ];

    /// The string form: the membership role stored in `org_members` and the
    /// `roles` the organization module accepts.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Admin => "admin",
            Role::Accountant => "accountant",
            Role::Approver => "approver",
            Role::Viewer => "viewer",
            Role::Auditor => "auditor",
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Role {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Role::ALL
            .into_iter()
            .find(|role| role.as_str() == s)
            .ok_or_else(|| format!("unknown role: {s}"))
    }
}

/// A principal: either a person with a company role, or an agent with a tier
/// and limits.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub enum Principal {
    /// A signed-in person (a human), with the role they hold in the company.
    Person {
        /// The person's subject id.
        person_id: String,
        /// The membership role the policy checks every command against.
        role: Role,
    },
    /// An agent key.
    Agent {
        /// The key's id.
        key_id: String,
        /// The key's tier.
        tier: Tier,
        /// Per-currency limits: base and transaction ceilings.
        limits: Limits,
    },
}

impl Principal {
    /// Converts this principal to a ledgers-audit Actor for the audit log.
    #[must_use]
    pub fn to_actor(&self) -> ledgers_audit::Actor {
        match self {
            Principal::Person { person_id, .. } => ledgers_audit::Actor::Person {
                person_id: person_id.clone(),
            },
            Principal::Agent { key_id, tier, .. } => ledgers_audit::Actor::Agent {
                key_id: key_id.clone(),
                tier: tier.as_str().to_owned(),
            },
        }
    }

    /// Returns the tier of this principal, or None for a person (who has no
    /// tier constraint).
    #[must_use]
    pub fn tier(&self) -> Option<Tier> {
        match self {
            Principal::Person { .. } => None,
            Principal::Agent { tier, .. } => Some(*tier),
        }
    }

    /// Returns the limits of this principal, or an empty map for a person.
    #[must_use]
    pub fn limits(&self) -> &Limits {
        if self.is_owner() {
            &EMPTY_LIMITS
        } else {
            match self {
                Principal::Agent { limits, .. } => limits,
                Principal::Person { .. } => unreachable!(),
            }
        }
    }

    /// Returns true if this principal is the owner (a person).
    #[must_use]
    pub fn is_owner(&self) -> bool {
        matches!(self, Principal::Person { .. })
    }

    /// Returns true if this principal is an agent.
    #[must_use]
    pub fn is_agent(&self) -> bool {
        matches!(self, Principal::Agent { .. })
    }
}

/// Empty limits for persons (who have no currency restrictions).
static EMPTY_LIMITS: Limits = Limits {
    inner: BTreeMap::new(),
};

/// An opaque credential (e.g., an API bearer token, a session ID).
pub type Credential = String;

/// Trait for authenticating a credential into a principal.
///
/// Implementations are async because they often query the database or
/// an external service. The futures are `Send`: the executor that
/// awaits them runs inside the server's axum handlers.
#[async_trait::async_trait]
pub trait Authenticator: Send + Sync {
    /// Authenticates a credential, returning a principal or an
    /// unauthenticated error.
    async fn authenticate(
        &self,
        credential: &Credential,
    ) -> Result<Principal, crate::error::CommandError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_ordering() {
        assert!(Tier::Read < Tier::Draft);
        assert!(Tier::Draft < Tier::PostWithinLimits);
    }

    #[test]
    fn tier_strings() {
        assert_eq!(Tier::Read.as_str(), "read");
        assert_eq!(Tier::Draft.as_str(), "draft");
        assert_eq!(Tier::PostWithinLimits.as_str(), "post-within-limits");
    }

    #[test]
    fn principal_actor_conversion() {
        let person = Principal::Person {
            person_id: "alice".to_owned(),
            role: Role::Owner,
        };
        assert_eq!(
            person.to_actor(),
            ledgers_audit::Actor::Person {
                person_id: "alice".to_owned(),
            }
        );

        let agent = Principal::Agent {
            key_id: "key-1".to_owned(),
            tier: Tier::Draft,
            limits: Limits::new(),
        };
        assert_eq!(
            agent.to_actor(),
            ledgers_audit::Actor::Agent {
                key_id: "key-1".to_owned(),
                tier: "draft".to_owned(),
            }
        );
    }

    #[test]
    fn limits_construction_rejects_invalid_code() {
        let limits = Limits::new().insert(
            "INVALID",
            Limit {
                base_ceiling: 1000,
                transaction_ceiling: 2000,
            },
        );
        assert!(limits.is_err());
        assert!(limits.unwrap_err().contains("invalid currency code"));
    }

    #[test]
    fn limits_construction_accepts_valid_code() {
        let limits = Limits::new().insert(
            "EUR",
            Limit {
                base_ceiling: 1000,
                transaction_ceiling: 2000,
            },
        );
        assert!(limits.is_ok());
        let limits = limits.unwrap();
        assert_eq!(limits.len(), 1);
        assert!(limits.contains_key("EUR"));
    }

    #[test]
    fn limits_deserialize_rejects_invalid_code() {
        let json = r#"{"INVALID": {"base_ceiling": 1000, "transaction_ceiling": 2000}}"#;
        let result: Result<Limits, _> = serde_json::from_str(json);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("invalid currency code")
        );
    }

    #[test]
    fn limits_deserialize_accepts_valid_code() {
        let json = r#"{"EUR": {"base_ceiling": 1000, "transaction_ceiling": 2000}}"#;
        let result: Result<Limits, _> = serde_json::from_str(json);
        assert!(result.is_ok());
        let limits = result.unwrap();
        assert_eq!(limits.len(), 1);
        assert!(limits.contains_key("EUR"));
    }

    #[test]
    fn limits_roundtrip_valid_code() {
        let original = Limits::new()
            .insert(
                "USD",
                Limit {
                    base_ceiling: 5000,
                    transaction_ceiling: 3000,
                },
            )
            .unwrap();

        let json = serde_json::to_string(&original).expect("serialize");
        let deserialized: Limits = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(original, deserialized);
    }

    #[test]
    fn limits_empty_is_valid() {
        let limits = Limits::new();
        assert!(limits.is_empty());

        let json = serde_json::to_string(&limits).expect("serialize");
        let deserialized: Limits = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(limits, deserialized);
    }
}
