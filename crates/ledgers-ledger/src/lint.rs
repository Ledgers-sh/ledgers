//! The chart linter: structural checks a template must pass before it may
//! be imported into any company's books. Pure functions over
//! [`ChartTemplate`] — no I/O — so a data author can run them on a file
//! (and CI on the bundled files) without a database.
//!
//! Two layers of validation exist on purpose. *Parse-time* checks in
//! [`crate::chart`] reject data that cannot even be typed (unknown account
//! classes, unknown currencies, malformed tags); *lint-time* checks here
//! reject charts that parse but are structurally wrong (duplicates,
//! cycles, missing system accounts). The split keeps every parse error
//! carrying its offending value while every lint finding is a
//! [`Violation`] a human can fix in the TOML.

use crate::chart::{ChartAccount, ChartTemplate};
use crate::error::AccountsError;
use std::fmt;

/// The tags every chart must carry exactly once, on a leaf account: the
/// posting machinery needs somewhere to put rounding residuals, FX gains
/// and losses, unallocable suspense items and the closing balance, and it
/// finds those accounts by tag, not by number (numbers are per-chart).
pub const REQUIRED_SYSTEM_TAGS: [&str; 5] = [
    "rounding",
    "fx-gain",
    "fx-loss",
    "suspense",
    "retained-earnings",
];

/// What a linter found wrong with a chart. Every variant names the
/// offending data so the fix is mechanical.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ViolationKind {
    /// The chart declares no accounts.
    EmptyChart,
    /// Two accounts claim the same number (the chart's natural key).
    DuplicateNumber {
        /// The number claimed twice.
        number: String,
    },
    /// A number that is not 2..=10 ASCII digits.
    NumberFormat {
        /// The malformed number.
        number: String,
        /// A short, static description of what was wrong.
        reason: &'static str,
    },
    /// The chart mixes number widths (e.g. `1000` beside `01000`), which
    /// would break lexical = numeric ordering.
    MixedNumberLengths,
    /// An account names a parent that is not in the chart.
    UnknownParent {
        /// The child account's number.
        number: String,
        /// The parent number it referenced.
        parent: String,
    },
    /// Following parent links returns to an account already on the path.
    Cycle {
        /// The numbers on the cycle, in parent → child order.
        numbers: Vec<String>,
    },
    /// Two accounts share one translation key.
    DuplicateNameKey {
        /// The key claimed twice.
        key: String,
    },
    /// A name key outside the `account.` namespace.
    BadNameKey {
        /// The account carrying the key.
        number: String,
        /// The key as written.
        key: String,
    },
    /// The template's own name key is outside the `template.` namespace.
    BadTemplateNameKey {
        /// The key as written.
        key: String,
    },
    /// No account carries a required system tag.
    MissingSystemAccount {
        /// The tag nothing carries.
        tag: &'static str,
    },
    /// More than one account carries a required system tag.
    DuplicateSystemAccount {
        /// The tag carried more than once.
        tag: &'static str,
    },
    /// The one account carrying a required system tag has children, so it
    /// is a heading and cannot take postings.
    SystemAccountNotLeaf {
        /// The tag.
        tag: &'static str,
        /// The heading account carrying it.
        number: String,
    },
}

/// One linter finding: what went wrong, and on which account when that is
/// meaningful (structural findings like an empty chart have none).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Violation {
    /// What went wrong.
    pub kind: ViolationKind,
    /// The number of the account the finding is about, when there is one.
    pub account: Option<String>,
}

impl Violation {
    /// A finding about a specific account.
    fn on(kind: ViolationKind, number: impl Into<String>) -> Self {
        Self {
            kind,
            account: Some(number.into()),
        }
    }

    /// A finding about the chart as a whole.
    fn chart(kind: ViolationKind) -> Self {
        Self {
            kind,
            account: None,
        }
    }
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(number) = &self.account {
            write!(f, "account {number}: ")?;
        }
        write!(f, "{}", self.kind)
    }
}

impl fmt::Display for ViolationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyChart => write!(f, "the chart declares no accounts"),
            Self::DuplicateNumber { number } => write!(f, "duplicate account number {number}"),
            Self::NumberFormat { number, reason } => write!(f, "bad number {number}: {reason}"),
            Self::MixedNumberLengths => write!(f, "numbers of mixed length (one width per chart)"),
            Self::UnknownParent { number, parent } => {
                write!(
                    f,
                    "account {number} names parent {parent}, which is not in the chart"
                )
            }
            Self::Cycle { numbers } => write!(f, "parent cycle: {}", numbers.join(" -> ")),
            Self::DuplicateNameKey { key } => write!(f, "duplicate name key {key}"),
            Self::BadNameKey { number: _, key } => {
                write!(f, "name key {key} must start with \"account.\"")
            }
            Self::BadTemplateNameKey { key } => {
                write!(f, "template name key {key} must start with \"template.\"")
            }
            Self::MissingSystemAccount { tag } => {
                write!(f, "no account carries the required system tag \"{tag}\"")
            }
            Self::DuplicateSystemAccount { tag } => {
                write!(f, "more than one account carries the system tag \"{tag}\"")
            }
            Self::SystemAccountNotLeaf { tag, number } => write!(
                f,
                "the account carrying the system tag \"{tag}\" ({number}) has children, so it \
                 is a heading and cannot take postings"
            ),
        }
    }
}

/// Renders a violation list for an error message: one semicolon-joined
/// line per finding, prefixed by the count.
pub(crate) fn format_violations(violations: &[Violation]) -> String {
    let mut text = format!("{} violation(s)", violations.len());
    for violation in violations {
        text.push_str("; ");
        text.push_str(&violation.to_string());
    }
    text
}

/// Lints a parsed chart. Returns every finding, not just the first: a
/// data author fixes a file in one pass, so the linter reports everything
/// it can see. Import paths treat a non-empty result as fatal.
///
/// Checks, in order: the chart is non-empty; every number is 2..=10 ASCII
/// digits and unique; all numbers share one width; every parent reference
/// resolves inside the chart; the parent graph has no cycles; name keys
/// are in the `account.` namespace and unique; and each of
/// [`REQUIRED_SYSTEM_TAGS`] is carried by exactly one leaf account.
/// eBilanz positions are deliberately *not* checked for uniqueness — many
/// accounts may share one position by design (see
/// [`crate::chart::ChartAccount::ebilanz`]). (Tag *format*, currencies,
/// classes and normal balances are parse-time checks — the module docs.)
#[must_use]
pub fn lint_chart(chart: &ChartTemplate) -> Vec<Violation> {
    let mut violations = Vec::new();
    if chart.accounts.is_empty() {
        violations.push(Violation::chart(ViolationKind::EmptyChart));
        return violations;
    }

    if !chart.name.starts_with("template.") {
        violations.push(Violation::chart(ViolationKind::BadTemplateNameKey {
            key: chart.name.clone(),
        }));
    }
    lint_numbers(chart, &mut violations);
    lint_parents(chart, &mut violations);
    lint_name_keys(chart, &mut violations);
    lint_system_accounts(chart, &mut violations);
    violations
}

/// Numbers: well-formed, unique, one width per chart. The width check
/// runs on the raw strings so a chart that also has malformed numbers
/// reports both problems at once.
fn lint_numbers(chart: &ChartTemplate, violations: &mut Vec<Violation>) {
    let mut seen_numbers = Vec::with_capacity(chart.accounts.len());
    let mut widths = Vec::with_capacity(chart.accounts.len());
    for account in &chart.accounts {
        if let Err(err) = crate::account::Account::validate_number(&account.number) {
            let AccountsError::InvalidNumber { reason, .. } = &err else {
                unreachable!("validate_number returns InvalidNumber only");
            };
            violations.push(Violation::on(
                ViolationKind::NumberFormat {
                    number: account.number.clone(),
                    reason,
                },
                &account.number,
            ));
        }
        if seen_numbers.contains(&account.number) {
            violations.push(Violation::on(
                ViolationKind::DuplicateNumber {
                    number: account.number.clone(),
                },
                &account.number,
            ));
        }
        seen_numbers.push(account.number.clone());
        if !widths.contains(&account.number.len()) {
            widths.push(account.number.len());
        }
    }
    if widths.len() > 1 {
        violations.push(Violation::chart(ViolationKind::MixedNumberLengths));
    }
}

/// Parents: every reference resolves inside the chart, and the graph is
/// acyclic (a self-reference is a one-node cycle).
fn lint_parents(chart: &ChartTemplate, violations: &mut Vec<Violation>) {
    let numbers: Vec<&str> = chart.accounts.iter().map(|a| a.number.as_str()).collect();
    for account in &chart.accounts {
        if let Some(parent) = &account.parent
            && parent != &account.number
            && !numbers.contains(&parent.as_str())
        {
            violations.push(Violation::on(
                ViolationKind::UnknownParent {
                    number: account.number.clone(),
                    parent: parent.clone(),
                },
                &account.number,
            ));
        }
    }
    for cycle in find_cycles(chart) {
        violations.push(Violation::on(
            ViolationKind::Cycle {
                numbers: cycle.clone(),
            },
            cycle.first().map(String::as_str).unwrap_or_default(),
        ));
    }
}

/// Name keys: `account.` namespace, unique within the chart.
fn lint_name_keys(chart: &ChartTemplate, violations: &mut Vec<Violation>) {
    let mut seen_keys = Vec::with_capacity(chart.accounts.len());
    for account in &chart.accounts {
        if !crate::account::Account::name_key_is_valid(&account.name) {
            violations.push(Violation::on(
                ViolationKind::BadNameKey {
                    number: account.number.clone(),
                    key: account.name.clone(),
                },
                &account.number,
            ));
        }
        if seen_keys.contains(&account.name) {
            violations.push(Violation::on(
                ViolationKind::DuplicateNameKey {
                    key: account.name.clone(),
                },
                &account.number,
            ));
        }
        seen_keys.push(account.name.clone());
    }
}

/// System tags: exactly one leaf account per required tag — headings
/// cannot take the postings the machinery will make.
fn lint_system_accounts(chart: &ChartTemplate, violations: &mut Vec<Violation>) {
    for tag in REQUIRED_SYSTEM_TAGS {
        let carriers: Vec<&ChartAccount> = chart
            .accounts
            .iter()
            .filter(|account| account.tags.iter().any(|candidate| candidate == tag))
            .collect();
        match carriers.as_slice() {
            [] => violations.push(Violation::chart(ViolationKind::MissingSystemAccount {
                tag,
            })),
            [only] => {
                let number = only.number.clone();
                let has_children = chart
                    .accounts
                    .iter()
                    .any(|account| account.parent.as_deref() == Some(number.as_str()));
                if has_children {
                    violations.push(Violation::on(
                        ViolationKind::SystemAccountNotLeaf {
                            tag,
                            number: number.clone(),
                        },
                        number,
                    ));
                }
            }
            _ => violations.push(Violation::chart(ViolationKind::DuplicateSystemAccount {
                tag,
            })),
        }
    }
}

/// Finds parent-link cycles by walking each account's ancestor chain,
/// one reused path vector per start node, and reporting when the walk
/// returns to a node already on the path. Only well-formed steps are
/// followed — unknown parents are reported by the caller's own checks,
/// not as cycles. Each cycle is reported exactly once, by its smallest
/// member, whatever node the walk started from.
fn find_cycles(chart: &ChartTemplate) -> Vec<Vec<String>> {
    let by_number = chart.index();
    let mut cycles = Vec::new();
    for start in &chart.accounts {
        let mut path: Vec<String> = Vec::new();
        let mut current = start.number.as_str();
        loop {
            if let Some(position) = path.iter().position(|node| node == current) {
                // Only report the cycle when this start node is the
                // smallest member, so each cycle is emitted exactly once.
                if path[position] == start.number {
                    let cycle = &path[position..];
                    if cycle.iter().min() == Some(&start.number) {
                        cycles.push(cycle.to_vec());
                    }
                }
                break;
            }
            let Some(node) = by_number.get(current) else {
                break; // unknown parent: reported separately
            };
            path.push(current.to_owned());
            let Some(parent) = &node.parent else {
                break; // reached a root
            };
            current = parent.as_str();
        }
    }
    cycles
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::{AccountType, NormalBalance};

    /// Builds a minimal lint-clean chart the tests then break in exactly
    /// one way each: five leaves, one per required system tag, one shared
    /// 4-digit width, a proper `template.` name key.
    fn clean() -> ChartTemplate {
        let account = |number: &str, name: &str, tags: &[&str]| ChartAccount {
            number: number.to_owned(),
            name: format!("account.t.{name}"),
            account_type: AccountType::Asset,
            normal_balance: NormalBalance::Debit,
            parent: None,
            currency: None,
            tags: tags.iter().map(|t| (*t).to_owned()).collect(),
            ebilanz: None,
        };
        ChartTemplate {
            id: "t".to_owned(),
            name: "template.t.name".to_owned(),
            version: 1,
            accounts: vec![
                account("1000", "rounding", &["rounding"]),
                account("2000", "fx-gain", &["fx-gain"]),
                account("3000", "fx-loss", &["fx-loss"]),
                account("4000", "suspense", &["suspense"]),
                account("5000", "retained", &["retained-earnings"]),
            ],
        }
    }

    fn kinds(violations: &[Violation]) -> Vec<ViolationKind> {
        violations.iter().map(|v| v.kind.clone()).collect()
    }

    #[test]
    fn the_fixture_itself_lints_clean() {
        assert!(lint_chart(&clean()).is_empty());
    }

    #[test]
    fn empty_chart() {
        let mut chart = clean();
        chart.accounts.clear();
        assert_eq!(kinds(&lint_chart(&chart)), vec![ViolationKind::EmptyChart]);
    }

    #[test]
    fn duplicate_number() {
        let mut chart = clean();
        let mut last = chart.accounts.pop().expect("fixture has accounts");
        last.number = "1000".to_owned();
        chart.accounts.push(last);
        assert!(
            kinds(&lint_chart(&chart)).contains(&ViolationKind::DuplicateNumber {
                number: "1000".to_owned()
            })
        );
    }

    #[test]
    fn number_format() {
        let mut chart = clean();
        chart.accounts[0].number = "1x".to_owned();
        assert!(
            kinds(&lint_chart(&chart)).contains(&ViolationKind::NumberFormat {
                number: "1x".to_owned(),
                reason: "account numbers are ASCII digits only",
            })
        );
    }

    #[test]
    fn mixed_number_lengths() {
        let mut chart = clean();
        chart.accounts[0].number = "01000".to_owned(); // 5 wide among 4
        assert!(kinds(&lint_chart(&chart)).contains(&ViolationKind::MixedNumberLengths));
    }

    #[test]
    fn unknown_parent() {
        let mut chart = clean();
        chart.accounts[1].parent = Some("9999".to_owned());
        assert!(
            kinds(&lint_chart(&chart)).contains(&ViolationKind::UnknownParent {
                number: "2000".to_owned(),
                parent: "9999".to_owned(),
            })
        );
    }

    #[test]
    fn two_node_cycle() {
        let mut chart = clean();
        chart.accounts[0].parent = Some("2000".to_owned());
        chart.accounts[1].parent = Some("1000".to_owned());
        assert!(kinds(&lint_chart(&chart)).contains(&ViolationKind::Cycle {
            numbers: vec!["1000".to_owned(), "2000".to_owned()]
        }));
    }

    #[test]
    fn self_parent_is_a_one_node_cycle() {
        let mut chart = clean();
        chart.accounts[0].parent = Some("1000".to_owned());
        assert!(kinds(&lint_chart(&chart)).contains(&ViolationKind::Cycle {
            numbers: vec!["1000".to_owned()]
        }));
    }

    #[test]
    fn duplicate_name_key() {
        let mut chart = clean();
        chart.accounts[1].name = chart.accounts[0].name.clone();
        assert!(
            kinds(&lint_chart(&chart)).contains(&ViolationKind::DuplicateNameKey {
                key: "account.t.rounding".to_owned()
            })
        );
    }

    #[test]
    fn bad_name_key() {
        let mut chart = clean();
        chart.accounts[0].name = "cash".to_owned();
        assert!(
            kinds(&lint_chart(&chart)).contains(&ViolationKind::BadNameKey {
                number: "1000".to_owned(),
                key: "cash".to_owned(),
            })
        );
    }

    #[test]
    fn bad_template_name_key() {
        let mut chart = clean();
        chart.name = "Generic SME".to_owned();
        assert!(
            kinds(&lint_chart(&chart)).contains(&ViolationKind::BadTemplateNameKey {
                key: "Generic SME".to_owned()
            })
        );
    }

    #[test]
    fn missing_system_account() {
        let mut chart = clean();
        chart.accounts[0].tags.clear();
        assert!(
            kinds(&lint_chart(&chart))
                .contains(&ViolationKind::MissingSystemAccount { tag: "rounding" })
        );
    }

    #[test]
    fn duplicate_system_account() {
        let mut chart = clean();
        chart.accounts[1].tags.push("rounding".to_owned());
        assert!(
            kinds(&lint_chart(&chart))
                .contains(&ViolationKind::DuplicateSystemAccount { tag: "rounding" })
        );
    }

    #[test]
    fn system_account_must_be_a_leaf() {
        let mut chart = clean();
        let mut child = chart.accounts[1].clone();
        child.number = "1100".to_owned();
        child.name = "account.t.child".to_owned();
        child.parent = Some("1000".to_owned()); // 1000 carries "rounding"
        chart.accounts.push(child);
        assert!(
            kinds(&lint_chart(&chart)).contains(&ViolationKind::SystemAccountNotLeaf {
                tag: "rounding",
                number: "1000".to_owned(),
            })
        );
    }

    #[test]
    fn violation_display_strings_are_agent_readable() {
        let violation = Violation::on(
            ViolationKind::UnknownParent {
                number: "2000".to_owned(),
                parent: "9999".to_owned(),
            },
            "2000",
        );
        let text = violation.to_string();
        assert!(text.starts_with("account 2000: "), "{text}");
        assert!(text.contains("9999"), "{text}");
        assert!(format_violations(&[violation]).starts_with("1 violation(s);"),);
    }
}
