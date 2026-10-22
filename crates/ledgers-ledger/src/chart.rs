//! Chart templates: the country/shape-specific starting charts a company
//! imports, parsed from the TOML files under `templates/`.
//!
//! Parsing is two-stage. `toml` + serde deserialises into private raw
//! structs (every field a string, exactly the file's shape); the raw form
//! is then validated into the typed [`ChartTemplate`] — unknown account
//! classes, normal balances and currencies become errors carrying the
//! offending value, malformed tags and empty eBilanz keys are rejected,
//! and structural failure (duplicates, cycles, missing system accounts)
//! is left to [`crate::lint`], which reports every finding at once.
//!
//! The file schema (see `templates/generic-sme.toml`):
//!
//! ```toml
//! id = "generic-sme"                    # kebab-case template id
//! name = "template.generic_sme.name"    # i18n key, "template." prefix
//! version = 1                           # integer data version
//! [[accounts]]
//! number = "1000"
//! name = "account.generic_sme.cash"     # i18n key, "account." prefix
//! type = "asset"                        # asset|liability|equity|income|expense
//! normal = "debit"                      # optional, default from type
//! parent = "1000"                       # optional; absent = root
//! currency = "EUR"                      # optional ISO 4217; absent = base
//! tags = ["cash", "bank"]               # optional kebab-case tags
//! ebilanz = "bs.001"                    # optional non-empty position id
//! ```

use crate::account::{Account, AccountType, NormalBalance};
use crate::error::AccountsError;
use crate::lint::{self, Violation};
use ledgers_money::Currency;
use serde::Deserialize;
use std::collections::HashMap;

/// The raw file shape, before validation: every field exactly as written.
#[derive(Deserialize, Debug)]
struct RawChartTemplate {
    id: String,
    name: String,
    version: u32,
    #[serde(default)]
    accounts: Vec<RawChartAccount>,
}

/// One `[[accounts]]` table, raw.
#[derive(Deserialize, Debug)]
struct RawChartAccount {
    number: String,
    name: String,
    #[serde(rename = "type")]
    account_type: String,
    normal: Option<String>,
    parent: Option<String>,
    currency: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    ebilanz: Option<String>,
}

/// A parsed, type-valid chart template. Structural validity (uniqueness,
/// acyclicity, system accounts) is [`ChartTemplate::lint`]'s to judge.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ChartTemplate {
    /// The kebab-case template id (`"generic-sme"`, `"de-hgb"`).
    pub id: String,
    /// The template's own name key (`"template.generic_sme.name"`).
    pub name: String,
    /// The integer data version, bumped when the file's accounts change.
    pub version: u32,
    /// The accounts, in file order.
    pub accounts: Vec<ChartAccount>,
}

/// One account of a chart template, typed: mirrors the TOML fields with
/// `type` → `account_type` and `normal` → `normal_balance` resolved to
/// their enums (and `currency` to a `ledgers-money` `Currency`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ChartAccount {
    /// The account number, 2..=10 ASCII digits.
    pub number: String,
    /// The translatable name key (`"account."` prefix).
    pub name: String,
    /// The account class.
    pub account_type: AccountType,
    /// The balance side; the file's `normal`, defaulted from the class.
    pub normal_balance: NormalBalance,
    /// The parent number; `None` = root.
    pub parent: Option<String>,
    /// The account's own currency; `None` = company base currency.
    pub currency: Option<Currency>,
    /// Kebab-case tags.
    pub tags: Vec<String>,
    /// The eBilanz position identifier, used by the German chart. Many
    /// accounts may share one position: fiscal-statement lines aggregate
    /// account balances, so for example the Umsatzerlöse accounts 19 %,
    /// 7 %, EU and Drittland all report under the single position
    /// `is.netIncome.regular.operatingTC.grossTradingProfit.totalOutput.netSales`.
    /// Repeated positions are by design and are therefore not linted
    /// against (parse still rejects an *empty* position).
    pub ebilanz: Option<String>,
}

impl ChartAccount {
    /// Converts this template account into a stored [`Account`] for
    /// `company_id`, with `now` (ISO 8601 UTC) as both timestamps and
    /// `archived = false`. Number and tags were validated at parse time;
    /// this is a data mapping, not a re-validation.
    #[must_use]
    pub fn to_account(&self, company_id: &str, now: &str) -> Account {
        Account {
            company_id: company_id.to_owned(),
            number: self.number.clone(),
            name_key: self.name.clone(),
            display_name: None,
            account_type: self.account_type,
            normal_balance: self.normal_balance,
            currency: self.currency,
            parent: self.parent.clone(),
            tags: self.tags.clone(),
            archived: false,
            created_at: now.to_owned(),
            updated_at: now.to_owned(),
        }
    }
}

impl ChartTemplate {
    /// Parses and type-validates a chart template from its TOML text.
    ///
    /// Syntax problems surface as [`AccountsError::Template`]; a value
    /// that cannot be typed (unknown class, unknown normal balance,
    /// unknown currency, malformed tag, empty eBilanz key) as the
    /// value-carrying variants. Structural checks are NOT run here — call
    /// [`ChartTemplate::lint`] (or `crate::commands::import_template`,
    /// which runs it) before trusting a chart.
    ///
    /// # Errors
    /// See above; every variant names the offending value.
    pub fn parse(toml_text: &str) -> Result<Self, AccountsError> {
        let raw: RawChartTemplate = toml::from_str(toml_text)?;
        let mut accounts = Vec::with_capacity(raw.accounts.len());
        for raw_account in raw.accounts {
            let account_type: AccountType = raw_account.account_type.parse()?;
            let normal_balance = match raw_account.normal.as_deref() {
                None => account_type.default_normal_balance(),
                Some(text) => text.parse()?,
            };
            let currency =
                match raw_account.currency.as_deref() {
                    None => None,
                    Some(code) => Some(Currency::from_code(code).map_err(|_| {
                        AccountsError::UnknownCurrency {
                            code: code.to_owned(),
                        }
                    })?),
                };
            Account::validate_tags(&raw_account.tags)?;
            if raw_account.ebilanz.as_deref().is_some_and(str::is_empty) {
                return Err(AccountsError::EmptyEbilanzPosition {
                    number: raw_account.number,
                });
            }
            accounts.push(ChartAccount {
                number: raw_account.number,
                name: raw_account.name,
                account_type,
                normal_balance,
                parent: raw_account.parent,
                currency,
                tags: raw_account.tags,
                ebilanz: raw_account.ebilanz,
            });
        }
        Ok(Self {
            id: raw.id,
            name: raw.name,
            version: raw.version,
            accounts,
        })
    }

    /// Lints the chart: `Ok(())` when it may be imported, `Err` holding
    /// every [`Violation`] when it may not (the command layer wraps that
    /// list in [`AccountsError::TemplateRejected`]).
    ///
    /// # Errors
    /// The full violation list, never a single finding.
    pub fn lint(&self) -> Result<(), Vec<Violation>> {
        let violations = lint::lint_chart(self);
        if violations.is_empty() {
            Ok(())
        } else {
            Err(violations)
        }
    }

    /// The accounts indexed by number. Duplicate numbers collapse (the
    /// linter reports them); everything read-side uses the first.
    #[must_use]
    pub fn index(&self) -> HashMap<&str, &ChartAccount> {
        self.accounts
            .iter()
            .map(|account| (account.number.as_str(), account))
            .collect()
    }

    /// The accounts in parents-before-children order: repeatedly the
    /// accounts whose parent is absent or already emitted, keeping file
    /// order within a level. This is the order imports insert in, so a
    /// child never points at a parent that does not exist yet. A
    /// lint-clean chart is acyclic, so every account is emitted; if a
    /// cycle is present anyway, the cycle's members are appended in file
    /// order (the linter has already rejected the chart).
    #[must_use]
    pub fn in_dependency_order(&self) -> Vec<&ChartAccount> {
        let mut ordered = Vec::with_capacity(self.accounts.len());
        let mut placed: Vec<&str> = Vec::with_capacity(self.accounts.len());
        while ordered.len() < self.accounts.len() {
            let mut progressed = false;
            for account in &self.accounts {
                if placed.contains(&account.number.as_str()) {
                    continue;
                }
                let parent_ready = account
                    .parent
                    .as_deref()
                    .is_none_or(|parent| parent == account.number || placed.contains(&parent));
                if parent_ready {
                    ordered.push(account);
                    placed.push(account.number.as_str());
                    progressed = true;
                }
            }
            if !progressed {
                // A cycle: emit the stragglers in file order so callers
                // that skipped lint still get every account.
                ordered.extend(
                    self.accounts
                        .iter()
                        .filter(|account| !placed.contains(&account.number.as_str())),
                );
                break;
            }
        }
        ordered
    }

    /// Whether the chart contains `number`.
    #[must_use]
    pub fn contains(&self, number: &str) -> bool {
        self.accounts.iter().any(|account| account.number == number)
    }
}

/// Parses a chart template from TOML text; see [`ChartTemplate::parse`].
///
/// # Errors
/// Whatever [`ChartTemplate::parse`] returns.
pub fn parse_chart(toml_text: &str) -> Result<ChartTemplate, AccountsError> {
    ChartTemplate::parse(toml_text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The minimal lint-clean file: mirrors the schema block above with
    /// the five system tags on leaves.
    const MINIMAL: &str = r#"
id = "minimal"
name = "template.minimal.name"
version = 1

[[accounts]]
number = "0100"
name = "account.minimal.current-assets"
type = "asset"

[[accounts]]
number = "1000"
name = "account.minimal.rounding"
type = "expense"
tags = ["rounding"]

[[accounts]]
number = "2000"
name = "account.minimal.fx-gain"
type = "income"
tags = ["fx-gain"]

[[accounts]]
number = "3000"
name = "account.minimal.fx-loss"
type = "expense"
tags = ["fx-loss"]

[[accounts]]
number = "4000"
name = "account.minimal.suspense"
type = "liability"
tags = ["suspense"]

[[accounts]]
number = "5000"
name = "account.minimal.retained-earnings"
type = "equity"
tags = ["retained-earnings"]

[[accounts]]
number = "0110"
name = "account.minimal.bank"
type = "asset"
normal = "debit"
parent = "0100"
currency = "EUR"
tags = ["bank"]
ebilanz = "bs.110"
"#;

    #[test]
    fn parses_the_full_schema() {
        let chart = ChartTemplate::parse(MINIMAL).expect("valid file parses");
        assert_eq!(chart.id, "minimal");
        assert_eq!(chart.version, 1);
        assert_eq!(chart.accounts.len(), 7);
        let bank = chart.index().get("0110").copied().expect("bank present");
        assert_eq!(bank.account_type, AccountType::Asset);
        assert_eq!(bank.normal_balance, NormalBalance::Debit);
        assert_eq!(bank.parent.as_deref(), Some("0100"));
        assert_eq!(
            bank.currency,
            Some(Currency::from_code("EUR").expect("EUR"))
        );
        assert_eq!(bank.ebilanz.as_deref(), Some("bs.110"));
        assert!(bank.tags.iter().any(|tag| tag == "bank"));
    }

    #[test]
    fn unknown_type_normal_and_currency_name_the_value() {
        let bad_type = MINIMAL.replace("type = \"expense\"", "type = \"costs\"");
        assert_eq!(
            ChartTemplate::parse(&bad_type).unwrap_err(),
            AccountsError::UnknownAccountType {
                value: "costs".to_owned()
            }
        );
        let bad_normal = MINIMAL.replace("normal = \"debit\"", "normal = \"left\"");
        assert_eq!(
            ChartTemplate::parse(&bad_normal).unwrap_err(),
            AccountsError::UnknownNormalBalance {
                value: "left".to_owned()
            }
        );
        let bad_currency = MINIMAL.replace("currency = \"EUR\"", "currency = \"ZZZ\"");
        assert_eq!(
            ChartTemplate::parse(&bad_currency).unwrap_err(),
            AccountsError::UnknownCurrency {
                code: "ZZZ".to_owned()
            }
        );
    }

    #[test]
    fn missing_normal_defaults_from_the_type() {
        let chart = ChartTemplate::parse(MINIMAL).expect("valid file parses");
        let income = chart.index().get("2000").copied().expect("fx-gain present");
        assert_eq!(income.account_type, AccountType::Income);
        assert_eq!(income.normal_balance, NormalBalance::Credit);
    }

    #[test]
    fn malformed_tags_and_empty_ebilanz_are_parse_errors() {
        let bad_tag = MINIMAL.replace("tags = [\"bank\"]", "tags = [\"Bank\"]");
        assert_eq!(
            ChartTemplate::parse(&bad_tag).unwrap_err(),
            AccountsError::InvalidTag {
                tag: "Bank".to_owned()
            }
        );
        let empty_ebilanz = MINIMAL.replace("ebilanz = \"bs.110\"", "ebilanz = \"\"");
        assert_eq!(
            ChartTemplate::parse(&empty_ebilanz).unwrap_err(),
            AccountsError::EmptyEbilanzPosition {
                number: "0110".to_owned()
            }
        );
    }

    #[test]
    fn broken_toml_is_a_template_error() {
        let err = ChartTemplate::parse("id = unclosed").unwrap_err();
        assert!(matches!(err, AccountsError::Template(_)));
    }

    #[test]
    fn dependency_order_puts_parents_first() {
        let mut chart = ChartTemplate::parse(MINIMAL).expect("valid file parses");
        // Reverse the file order so passing requires real ordering.
        chart.accounts.reverse();
        let ordered = chart.in_dependency_order();
        let positions: Vec<&str> = ordered.iter().map(|a| a.number.as_str()).collect();
        let bank_at = positions
            .iter()
            .position(|number| *number == "0110")
            .expect("bank emitted");
        let parent_at = positions
            .iter()
            .position(|number| *number == "0100")
            .expect("parent emitted (root added below)");
        assert!(parent_at < bank_at, "parent 0100 must precede child 0110");
        assert_eq!(
            ordered.len(),
            chart.accounts.len(),
            "every account emitted once"
        );
    }

    #[test]
    fn dependency_order_survives_a_cycle_without_looping() {
        let mut chart = ChartTemplate::parse(MINIMAL).expect("valid file parses");
        chart.accounts[0].parent = Some("2000".to_owned());
        chart.accounts[1].parent = Some("1000".to_owned());
        let ordered = chart.in_dependency_order();
        assert_eq!(
            ordered.len(),
            chart.accounts.len(),
            "cycle members still emitted"
        );
    }

    #[test]
    fn template_account_converts_to_a_stored_account() {
        let chart = ChartTemplate::parse(MINIMAL).expect("valid file parses");
        let bank = chart.index().get("0110").copied().expect("bank present");
        let account = bank.to_account("co-1", "2026-02-01T09:00:00Z");
        assert_eq!(account.company_id, "co-1");
        assert_eq!(account.number, "0110");
        assert_eq!(account.name_key, "account.minimal.bank");
        assert_eq!(account.parent.as_deref(), Some("0100"));
        assert!(!account.archived);
        assert_eq!(account.created_at, "2026-02-01T09:00:00Z");
        assert_eq!(account.updated_at, "2026-02-01T09:00:00Z");
    }

    #[test]
    fn contains_and_lint_wire_up() {
        let chart = ChartTemplate::parse(MINIMAL).expect("valid file parses");
        assert!(chart.contains("0110"));
        assert!(!chart.contains("9999"));
        // The fixture declares every parent and all five system tags on
        // leaves: it lints clean. Break one link and lint must say so.
        assert!(chart.lint().is_ok());
        let mut broken = chart.clone();
        broken.accounts[6].parent = Some("9999".to_owned());
        assert!(broken.lint().is_err());
    }
}
