//! Agent key management for people: `keys.list`, `keys.create`, `keys.revoke`.
//!
//! These commands are human-only. [`crate::dispatch::ServerDispatch`] checks
//! the caller is a person before calling in here, so an agent key gets a 403
//! and nothing is read or written. The key itself has two halves: the policy
//! row in `agent_key` (name, tier, limits, owner, `created_at`) and the harness
//! `api_keys` row that holds the hash, the public prefix, `last_used_at` and
//! `revoked_at`. This module reads and writes both.
//!
//! Secrets: the full token is returned exactly once, from [`create`]. Every
//! other output carries the public prefix only, and nothing here logs a token.
//! Audit events come from `ledgers_auth::AgentKeys` (issue, revoke) and name
//! the public prefix, never the secret.

use cratefield_core::{Database, Problem, Statement};
use ledgers_audit::{Actor, AuditContext};
use ledgers_auth::AgentKeys;
use ledgers_commands::CommandError;
use ledgers_commands::auth::Tier;
use ledgers_money::{Amount, Currency};
use ledgers_policy::key::{AgentKey, CurrencyLimit};
use ledgers_policy::store::deserialize_key;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::dispatch::{self, Deps};

/// The longest key name, in characters.
pub const NAME_MAX_CHARS: usize = 80;

/// The API-key namespace agent keys are issued under (`lk_live_<id>_<secret>`).
const NAMESPACE: &str = ledgers_auth::keys::KEY_NAMESPACE;

/// Input of `keys.list`: none. The command takes no parameters.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
pub struct KeysListInput {}

/// One currency's limits, as canonical decimal strings in major units
/// (`"1000.00"` for EUR is 100000 minor units).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LimitAmounts {
    /// The most one entry may move in this currency.
    pub per_entry: String,
    /// The most the key may move in this currency in a rolling day.
    pub per_day: String,
}

/// Input of `keys.create`.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct KeysCreateInput {
    /// A label for the key, shown in the app and the audit trail.
    pub name: String,
    /// `read`, `draft` or `post-within-limits`.
    pub tier: String,
    /// Per-currency limits, keyed by ISO 4217 code. A currency with no entry
    /// is not allowed at all (fail closed).
    pub limits: BTreeMap<String, LimitAmounts>,
    /// Pin the key to one business unit: its entries carry that unit, and it
    /// may post to no other. Absent means all units.
    #[serde(default)]
    pub unit: Option<String>,
}

/// Input of `keys.revoke`: the key's `id`, as `keys.list` returns it.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct KeysRevokeInput {
    /// The key's id.
    pub id: String,
}

/// One agent key as the owner sees it. Never carries the secret.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentKeyView {
    /// The key's id; `keys.revoke` takes it.
    pub id: String,
    /// The label the key was created with.
    pub name: String,
    /// The business unit the key is pinned to; absent means all units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// The public prefix, `lk_live_<id>`. The secret part is never shown again.
    pub prefix: Option<String>,
    /// `read`, `draft` or `post-within-limits`.
    pub tier: String,
    /// Per-currency limits, in canonical major-unit strings.
    pub limits: BTreeMap<String, LimitAmounts>,
    /// When the key was created (RFC 3339).
    pub created_at: String,
    /// The person who created the key.
    pub created_by: String,
    /// When the key last authenticated a request, at most one write per minute.
    pub last_used_at: Option<String>,
    /// When the key was revoked, if it was.
    pub revoked_at: Option<String>,
}

/// Output of `keys.list`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct KeysListOutput {
    /// Every agent key of the company, revoked ones included.
    pub keys: Vec<AgentKeyView>,
}

/// Output of `keys.create`: the key, and its full token, shown once.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct KeysCreateOutput {
    /// The new key.
    pub key: AgentKeyView,
    /// The full bearer token. It is not stored and cannot be shown again.
    pub token: String,
}

/// Output of `keys.revoke`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct KeysRevokeOutput {
    /// The key, with `revoked_at` set.
    pub key: AgentKeyView,
}

/// The tier a key may be created with, from its wire spelling.
///
/// # Errors
/// [`CommandError::InvalidInput`] for any other spelling.
pub fn parse_tier(text: &str) -> Result<Tier, CommandError> {
    match text {
        "read" => Ok(Tier::Read),
        "draft" => Ok(Tier::Draft),
        "post-within-limits" => Ok(Tier::PostWithinLimits),
        other => Err(CommandError::InvalidInput(format!(
            "tier must be read, draft or post-within-limits, got {other:?}"
        ))),
    }
}

/// Checks a key name and returns it trimmed.
///
/// # Errors
/// [`CommandError::InvalidInput`] when it is empty or too long.
pub fn validate_name(name: &str) -> Result<String, CommandError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(CommandError::InvalidInput("name is required".to_owned()));
    }
    if trimmed.chars().count() > NAME_MAX_CHARS {
        return Err(CommandError::InvalidInput(format!(
            "name is longer than {NAME_MAX_CHARS} characters"
        )));
    }
    Ok(trimmed.to_owned())
}

/// Converts the wire limits to policy limits: each amount is parsed in its
/// currency's own exponent, so `"1000.5"` EUR becomes 100050 minor units.
///
/// # Errors
/// [`CommandError::InvalidInput`] for an unknown currency, an amount that is
/// not a canonical decimal in that currency's exponent, or a negative amount.
pub fn parse_limits(
    limits: &BTreeMap<String, LimitAmounts>,
) -> Result<BTreeMap<String, CurrencyLimit>, CommandError> {
    let mut out = BTreeMap::new();
    for (code, amounts) in limits {
        let currency = Currency::from_code(code)
            .map_err(|err| CommandError::InvalidInput(format!("currency {code:?}: {err}")))?;
        let per_entry = parse_minor(&amounts.per_entry, currency, "per_entry", code)?;
        let per_day = parse_minor(&amounts.per_day, currency, "per_day", code)?;
        let limit = CurrencyLimit { per_entry, per_day };
        limit
            .validate()
            .map_err(|err| CommandError::InvalidInput(err.to_string()))?;
        out.insert(currency.code().to_owned(), limit);
    }
    Ok(out)
}

fn parse_minor(
    text: &str,
    currency: Currency,
    field: &str,
    code: &str,
) -> Result<i128, CommandError> {
    Amount::parse_canonical(text.trim(), currency)
        .map(Amount::minor)
        .map_err(|err| CommandError::InvalidInput(format!("{field} for {code}: {err}")))
}

/// Lists every agent key of `company_id`, revoked ones included.
///
/// # Errors
/// Database or stored-policy failures.
pub async fn list(deps: &Deps, company_id: &str) -> Result<Vec<AgentKeyView>, CommandError> {
    load_views(deps.db.as_ref(), company_id, None).await
}

/// Creates an agent key owned by `person_id` and returns its full token once.
///
/// # Errors
/// [`CommandError::InvalidInput`] for a bad name, tier or limit; a domain
/// problem when the key cannot be stored.
pub async fn create(
    deps: &Deps,
    company_id: &str,
    person_id: &str,
    input: &KeysCreateInput,
) -> Result<KeysCreateOutput, CommandError> {
    let name = validate_name(&input.name)?;
    let tier = parse_tier(&input.tier)?;
    let limits = parse_limits(&input.limits)?;
    let unit = resolve_pin(deps.db.as_ref(), company_id, input.unit.as_deref()).await?;
    let (key_id, token) =
        dispatch::issue_key_as(deps, company_id, &name, person_id, tier, limits, unit)
            .await
            .map_err(|err| {
                // The failed step names a table, never a secret; it stays in the log.
                tracing::error!(error = %err, "agent key creation failed");
                CommandError::Domain(Problem::internal())
            })?;
    let key = load_views(deps.db.as_ref(), company_id, Some(&key_id))
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| CommandError::Domain(Problem::internal()))?;
    Ok(KeysCreateOutput { key, token })
}

/// Checks the unit a key is pinned to: it must name an active unit of the
/// company. `None` pins nothing (the key may post to every unit).
///
/// # Errors
/// The unit problems of [`crate::dispatch::resolve_scope`]'s counterpart:
/// `unit-not-found` for an unknown code, `unit-conflict` for an archived one.
pub async fn resolve_pin(
    db: &dyn cratefield_core::Database,
    company_id: &str,
    unit: Option<&str>,
) -> Result<Option<String>, CommandError> {
    let Some(code) = unit.map(str::trim).filter(|code| !code.is_empty()) else {
        return Ok(None);
    };
    let code = ledgers_ledger::units::validate_code(code)
        .map_err(|err| CommandError::Domain(err.to_problem()))?;
    let found = ledgers_ledger::units::find_unit(db, company_id, &code)
        .await
        .map_err(|err| CommandError::Domain(err.to_problem()))?
        .ok_or_else(|| {
            CommandError::Domain(
                ledgers_ledger::AccountsError::UnitNotFound { code: code.clone() }.to_problem(),
            )
        })?;
    if found.archived {
        return Err(CommandError::Domain(
            ledgers_ledger::AccountsError::UnitArchived { code }.to_problem(),
        ));
    }
    Ok(Some(code))
}

/// Revokes the agent key `id`. Revoking a key that is already revoked changes
/// nothing and returns the key as it is.
///
/// Revocation is immediate: the harness `ApiKeys` row gets `revoked_at`, and
/// every later verification of that key (HTTP and MCP alike) answers
/// unauthenticated.
///
/// # Errors
/// `not-found` when no key of the company has that id.
pub async fn revoke(
    deps: &Deps,
    company_id: &str,
    person_id: &str,
    id: &str,
) -> Result<KeysRevokeOutput, CommandError> {
    let db = deps.db.as_ref();
    let keys = key_rows(db, company_id, id).await?;
    if keys.is_empty() {
        return Err(CommandError::Domain(Problem::not_found()));
    }
    if let Some(active) = keys.iter().find(|row| row.revoked_at.is_none()) {
        let audit = AuditContext::new(
            Actor::Person {
                person_id: person_id.to_owned(),
            },
            deps.id_gen.ulid(),
            deps.id_gen.ulid(),
        );
        let agent_keys = AgentKeys::new(
            Arc::new(dispatch::api_keys(&deps.db, &deps.clock)),
            deps.db.clone(),
        );
        agent_keys
            .revoke(
                &active.prefix,
                company_id,
                &audit,
                &dispatch::rfc3339_utc(deps.clock.now()),
            )
            .await
            .map_err(|err| {
                tracing::error!(error = %err, "agent key revocation failed");
                CommandError::Domain(Problem::internal())
            })?;
    }
    let key = load_views(db, company_id, Some(id))
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| CommandError::Domain(Problem::not_found()))?;
    Ok(KeysRevokeOutput { key })
}

/// One `api_keys` row of a policy row, as the key-management queries read it.
struct KeyRow {
    prefix: String,
    revoked_at: Option<String>,
}

/// The harness key rows of agent key `id` in `company_id`, newest first.
async fn key_rows(
    db: &dyn Database,
    company_id: &str,
    id: &str,
) -> Result<Vec<KeyRow>, CommandError> {
    let rows = db
        .query(&Statement::with_values(
            format!(
                "SELECT k.prefix AS prefix, k.revoked_at AS revoked_at FROM agent_key a \
                 JOIN {table} k ON k.subject = a.id AND k.namespace = ? \
                 WHERE a.company_id = ? AND a.id = ? ORDER BY k.created_at DESC",
                table = dispatch::API_KEYS_TABLE
            ),
            vec![
                sea_query::Value::from(NAMESPACE),
                sea_query::Value::from(company_id),
                sea_query::Value::from(id),
            ],
        ))
        .await?;
    Ok(rows
        .rows
        .iter()
        .filter_map(|row| {
            Some(KeyRow {
                prefix: row.get::<String>("prefix")?,
                revoked_at: row.get::<Option<String>>("revoked_at").flatten(),
            })
        })
        .collect())
}

/// Reads the policy rows of `company_id` (only `only`, when given), joined to
/// their harness key rows.
async fn load_views(
    db: &dyn Database,
    company_id: &str,
    only: Option<&str>,
) -> Result<Vec<AgentKeyView>, CommandError> {
    let mut sql = format!(
        "SELECT a.id AS id, a.policy AS policy, k.prefix AS prefix, \
         k.last_used_at AS last_used_at, k.revoked_at AS api_revoked_at, k.created_at AS api_created_at \
         FROM agent_key a LEFT JOIN {table} k ON k.subject = a.id AND k.namespace = ? \
         WHERE a.company_id = ?",
        table = dispatch::API_KEYS_TABLE
    );
    let mut values = vec![
        sea_query::Value::from(NAMESPACE),
        sea_query::Value::from(company_id),
    ];
    if let Some(id) = only {
        sql.push_str(" AND a.id = ?");
        values.push(sea_query::Value::from(id));
    }
    sql.push_str(" ORDER BY a.id, k.created_at DESC");
    let rows = db.query(&Statement::with_values(sql, values)).await?;

    // A key has one harness row unless it was rotated; prefer the active one.
    let mut by_id: BTreeMap<String, AgentKeyView> = BTreeMap::new();
    for row in &rows.rows {
        let Some(id) = row.get::<String>("id") else {
            continue;
        };
        let Some(json) = row.get::<String>("policy") else {
            continue;
        };
        let policy = deserialize_key(&json)
            .map_err(|err| CommandError::InvalidInput(format!("agent key policy: {err}")))?;
        let prefix = row.get::<Option<String>>("prefix").flatten();
        let api_revoked = row.get::<Option<String>>("api_revoked_at").flatten();
        let is_active = api_revoked.is_none();
        let last_used_at = row.get::<Option<String>>("last_used_at").flatten();
        let view = view_of(&id, &policy, prefix, last_used_at, api_revoked)?;
        let replace = match by_id.get(&id) {
            None => true,
            Some(existing) => existing.revoked_at.is_some() && is_active,
        };
        if replace {
            by_id.insert(id, view);
        }
    }
    Ok(by_id.into_values().collect())
}

fn view_of(
    id: &str,
    policy: &AgentKey,
    prefix: Option<String>,
    last_used_at: Option<String>,
    revoked_at: Option<String>,
) -> Result<AgentKeyView, CommandError> {
    let mut limits = BTreeMap::new();
    for (code, limit) in &policy.limits {
        let currency = Currency::from_code(code).map_err(|err| {
            CommandError::InvalidInput(format!("stored currency {code:?}: {err}"))
        })?;
        limits.insert(
            code.clone(),
            LimitAmounts {
                per_entry: Amount::new(limit.per_entry, currency).to_canonical_string(),
                per_day: Amount::new(limit.per_day, currency).to_canonical_string(),
            },
        );
    }
    Ok(AgentKeyView {
        id: id.to_owned(),
        name: policy.label.clone(),
        unit: policy.unit.clone(),
        prefix,
        tier: policy.tier.as_str().to_owned(),
        limits,
        created_at: policy
            .created_at
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
        created_by: policy.owner.clone(),
        last_used_at,
        revoked_at: revoked_at.or_else(|| {
            policy.revoked_at.and_then(|at| {
                at.format(&time::format_description::well_known::Rfc3339)
                    .ok()
            })
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn amounts(per_entry: &str, per_day: &str) -> LimitAmounts {
        LimitAmounts {
            per_entry: per_entry.to_owned(),
            per_day: per_day.to_owned(),
        }
    }

    #[test]
    fn tiers_parse_only_the_three_spellings() {
        assert_eq!(parse_tier("read").unwrap(), Tier::Read);
        assert_eq!(parse_tier("draft").unwrap(), Tier::Draft);
        assert_eq!(
            parse_tier("post-within-limits").unwrap(),
            Tier::PostWithinLimits
        );
        assert!(parse_tier("admin").is_err());
        assert!(parse_tier("Read").is_err());
    }

    #[test]
    fn names_are_trimmed_and_bounded() {
        assert_eq!(validate_name("  Bank bot ").unwrap(), "Bank bot");
        assert!(validate_name("   ").is_err());
        assert!(validate_name(&"x".repeat(NAME_MAX_CHARS + 1)).is_err());
        assert!(validate_name(&"x".repeat(NAME_MAX_CHARS)).is_ok());
    }

    #[test]
    fn limits_are_parsed_in_the_currency_exponent() {
        let input = BTreeMap::from([
            ("EUR".to_owned(), amounts("1000.5", "10000")),
            ("JPY".to_owned(), amounts("500", "5000")),
            ("BHD".to_owned(), amounts("1.234", "2")),
        ]);
        let limits = parse_limits(&input).expect("parses");
        assert_eq!(limits["EUR"].per_entry, 100_050);
        assert_eq!(limits["EUR"].per_day, 1_000_000);
        assert_eq!(limits["JPY"].per_entry, 500);
        assert_eq!(limits["BHD"].per_entry, 1_234);
        assert_eq!(limits["BHD"].per_day, 2_000);
    }

    #[test]
    fn limits_reject_floats_negatives_extra_precision_and_unknown_currencies() {
        for bad in ["1000.555", "-1", "1e3", "1,000", "", "abc"] {
            let input = BTreeMap::from([("EUR".to_owned(), amounts(bad, "1"))]);
            assert!(parse_limits(&input).is_err(), "{bad:?} must be refused");
        }
        let unknown = BTreeMap::from([("XXX1".to_owned(), amounts("1", "1"))]);
        assert!(parse_limits(&unknown).is_err());
    }
}
