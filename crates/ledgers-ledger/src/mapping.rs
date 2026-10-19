//! SKR03/SKR04 → Ledgers account-number mappings.
//!
//! German businesses arrive with books kept in Datev's SKR03 or SKR04
//! charts. Migrating them means translating their account *numbers* into
//! the numbers of the Ledgers chart a company imports. Per the licence
//! decision in issue #5, the mapping files carry **numbers only** — SKR
//! account *names* are Datev's intellectual property and are deliberately
//! absent from this repository.
//!
//! The file schema (see `templates/mappings/de-skr03.toml`):
//!
//! ```toml
//! id = "de-skr03-to-de-hgb"
//! source = "de-skr03"      # external chart identifier — numbers only
//! target = "de-hgb"        # template id the numbers map into
//! version = 1
//! [[map]]
//! from = "0120"            # external (SKR) account number, 4 ASCII digits
//! to   = "11100"           # Ledgers account number in the target chart
//! ```

use crate::chart::ChartTemplate;
use crate::error::AccountsError;
use serde::Deserialize;
use std::collections::HashSet;
use std::fmt;

/// The SKR03 → de-hgb mapping (`templates/mappings/de-skr03.toml`).
pub const SKR03_MAPPING_TOML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/mappings/de-skr03.toml"
));
/// The SKR04 → de-hgb mapping (`templates/mappings/de-skr04.toml`).
pub const SKR04_MAPPING_TOML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/mappings/de-skr04.toml"
));

/// The raw file shape, before validation.
#[derive(Deserialize, Debug)]
struct RawNumberMapping {
    id: String,
    source: String,
    target: String,
    version: u32,
    #[serde(default)]
    map: Vec<RawMapEntry>,
}

/// One `[[map]]` table, raw.
#[derive(Deserialize, Debug)]
struct RawMapEntry {
    from: String,
    to: String,
}

/// A parsed number mapping: from an external chart's numbers into one
/// target template's numbers.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NumberMapping {
    /// The mapping's own id (`"de-skr03-to-de-hgb"`).
    pub id: String,
    /// The external chart identifier (`"de-skr03"`). Numbers only — see
    /// the module docs for why no SKR names appear.
    pub source: String,
    /// The target template id (`"de-hgb"`).
    pub target: String,
    /// The integer data version.
    pub version: u32,
    /// The number pairs, in file order.
    pub map: Vec<MapEntry>,
}

/// One number pair: an external chart's number → a target-chart number.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MapEntry {
    /// The external account number (4 ASCII digits for SKR charts).
    pub from: String,
    /// The target-chart account number.
    pub to: String,
}

/// What makes a mapping invalid against its target chart.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MappingViolationKind {
    /// A `from` number that is not 4 ASCII digits.
    BadFrom {
        /// The malformed number.
        from: String,
        /// A short, static description of what was wrong.
        reason: &'static str,
    },
    /// The same `from` number mapped twice.
    DuplicateFrom {
        /// The number mapped twice.
        from: String,
    },
    /// A `to` number that does not exist in the target chart.
    UnknownTarget {
        /// The source number being mapped.
        from: String,
        /// The target-chart number that is absent.
        to: String,
    },
}

/// One mapping-validation finding.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MappingViolation {
    /// What went wrong.
    pub kind: MappingViolationKind,
}

impl fmt::Display for MappingViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.kind)
    }
}

impl fmt::Display for MappingViolationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadFrom { from, reason } => {
                write!(f, "bad source number {from}: {reason}")
            }
            Self::DuplicateFrom { from } => write!(f, "source number {from} mapped twice"),
            Self::UnknownTarget { from, to } => write!(
                f,
                "entry {from} -> {to} points at a number the target chart does not declare"
            ),
        }
    }
}

/// Renders a mapping-violation list for an error message.
pub(crate) fn format_mapping_violations(violations: &[MappingViolation]) -> String {
    let mut text = format!("{} violation(s)", violations.len());
    for violation in violations {
        text.push_str("; ");
        text.push_str(&violation.to_string());
    }
    text
}

/// Parses a number mapping from its TOML text. Syntax problems surface as
/// [`AccountsError::Template`]; semantic validation (digit format,
/// uniqueness, target existence) is [`NumberMapping::validate_against`].
///
/// # Errors
/// [`AccountsError::Template`] when the file is not valid TOML for the
/// schema.
pub fn parse_mapping(toml_text: &str) -> Result<NumberMapping, AccountsError> {
    let raw: RawNumberMapping = toml::from_str(toml_text)?;
    Ok(NumberMapping {
        id: raw.id,
        source: raw.source,
        target: raw.target,
        version: raw.version,
        map: raw
            .map
            .into_iter()
            .map(|entry| MapEntry {
                from: entry.from,
                to: entry.to,
            })
            .collect(),
    })
}

impl NumberMapping {
    /// Validates the mapping against the chart it claims to map into:
    /// every `from` is 4 ASCII digits and unique, and every `to` exists
    /// in `chart`. All findings are returned together (callers wrap the
    /// list in [`AccountsError::MappingRejected`]).
    ///
    /// # Errors
    /// The full violation list, never a single finding.
    pub fn validate_against(&self, chart: &ChartTemplate) -> Result<(), Vec<MappingViolation>> {
        let mut violations = Vec::new();
        let mut seen: HashSet<&str> = HashSet::with_capacity(self.map.len());
        for entry in &self.map {
            if entry.from.len() != 4 || !entry.from.bytes().all(|b| b.is_ascii_digit()) {
                violations.push(MappingViolation {
                    kind: MappingViolationKind::BadFrom {
                        from: entry.from.clone(),
                        reason: "SKR source numbers are 4 ASCII digits",
                    },
                });
            }
            if !seen.insert(&entry.from) {
                violations.push(MappingViolation {
                    kind: MappingViolationKind::DuplicateFrom {
                        from: entry.from.clone(),
                    },
                });
            }
            if !chart.contains(&entry.to) {
                violations.push(MappingViolation {
                    kind: MappingViolationKind::UnknownTarget {
                        from: entry.from.clone(),
                        to: entry.to.clone(),
                    },
                });
            }
        }
        if violations.is_empty() {
            Ok(())
        } else {
            Err(violations)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A target chart with the numbers `11100` and `41000` only.
    fn target_chart() -> ChartTemplate {
        crate::chart::ChartTemplate::parse(
            r#"
id = "target"
name = "template.target.name"
version = 1

[[accounts]]
number = "11100"
name = "account.target.bank"
type = "asset"

[[accounts]]
number = "41000"
name = "account.target.sales"
type = "income"
"#,
        )
        .expect("target chart parses")
    }

    const GOOD: &str = r#"
id = "src-to-target"
source = "src-skr"
target = "target"
version = 2

[[map]]
from = "0120"
to = "11100"

[[map]]
from = "8400"
to = "41000"
"#;

    #[test]
    fn parses_the_schema() {
        let mapping = parse_mapping(GOOD).expect("valid file parses");
        assert_eq!(mapping.id, "src-to-target");
        assert_eq!(mapping.source, "src-skr");
        assert_eq!(mapping.target, "target");
        assert_eq!(mapping.version, 2);
        assert_eq!(
            mapping.map,
            vec![
                MapEntry {
                    from: "0120".to_owned(),
                    to: "11100".to_owned()
                },
                MapEntry {
                    from: "8400".to_owned(),
                    to: "41000".to_owned()
                },
            ]
        );
    }

    #[test]
    fn a_good_mapping_validates() {
        let mapping = parse_mapping(GOOD).expect("valid file parses");
        assert!(mapping.validate_against(&target_chart()).is_ok());
    }

    #[test]
    fn bad_from_duplicate_from_and_unknown_target_all_reported() {
        let bad = r#"
id = "src-to-target"
source = "src-skr"
target = "target"
version = 1

[[map]]
from = "120"
to = "11100"

[[map]]
from = "120"
to = "11100"

[[map]]
from = "8400"
to = "99999"
"#;
        let mapping = parse_mapping(bad).expect("file parses; it is semantics that fail");
        let violations = mapping.validate_against(&target_chart()).unwrap_err();
        assert!(violations.iter().any(|v| v.kind
            == MappingViolationKind::BadFrom {
                from: "120".to_owned(),
                reason: "SKR source numbers are 4 ASCII digits",
            }));
        assert!(violations.iter().any(|v| v.kind
            == MappingViolationKind::DuplicateFrom {
                from: "120".to_owned()
            }));
        assert!(violations.iter().any(|v| v.kind
            == MappingViolationKind::UnknownTarget {
                from: "8400".to_owned(),
                to: "99999".to_owned(),
            }));
        assert_eq!(violations.len(), 4, "every finding at once: {violations:?}");
    }

    #[test]
    fn broken_toml_is_a_template_error() {
        assert!(matches!(
            parse_mapping("id = "),
            Err(AccountsError::Template(_))
        ));
    }

    #[test]
    fn violation_display_is_agent_readable() {
        let violation = MappingViolation {
            kind: MappingViolationKind::UnknownTarget {
                from: "8400".to_owned(),
                to: "99999".to_owned(),
            },
        };
        let text = violation.to_string();
        assert!(text.contains("8400") && text.contains("99999"), "{text}");
        let duplicate = MappingViolation {
            kind: MappingViolationKind::DuplicateFrom {
                from: "120".to_owned(),
            },
        };
        assert!(format_mapping_violations(&[violation, duplicate]).starts_with("2 violation(s);"));
    }
}
