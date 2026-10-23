//! Allocation rules and the needs-mapping queue (issue #142).
//!
//! A rule maps a vendor resource id to a business unit. A pattern is either
//! an exact resource id or a prefix ending in `*`. A resource with no rule
//! books to the connection's default unit and is listed as needing a
//! mapping, so nothing is silently lost to the wrong venture.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One rule: a resource id pattern and the unit it books to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AllocationRule {
    /// An exact resource id, or a prefix followed by `*`.
    pub pattern: String,
    /// The business unit code.
    pub unit_code: String,
}

/// Whether `pattern` matches `resource_id`.
#[must_use]
pub fn pattern_matches(pattern: &str, resource_id: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => resource_id.starts_with(prefix),
        None => pattern == resource_id,
    }
}

/// The unit for a resource: the first matching rule, or `None`.
#[must_use]
pub fn unit_for<'a>(rules: &'a [AllocationRule], resource_id: &str) -> Option<&'a str> {
    rules
        .iter()
        .find(|rule| pattern_matches(&rule.pattern, resource_id))
        .map(|rule| rule.unit_code.as_str())
}

/// Checks a rule set before it is stored: patterns are non-empty, at most one
/// `*` at the end, and unit codes are short.
///
/// # Errors
/// A sentence naming the bad rule.
pub fn validate_rules(rules: &[AllocationRule]) -> Result<(), String> {
    for rule in rules {
        let pattern = rule.pattern.trim();
        if pattern.is_empty() || pattern.len() > 200 {
            return Err("an allocation pattern must be 1 to 200 characters".to_owned());
        }
        if pattern.matches('*').count() > 1 || (pattern.contains('*') && !pattern.ends_with('*')) {
            return Err(format!("{pattern}: only a trailing * is a wildcard"));
        }
        if rule.unit_code.trim().is_empty() {
            return Err(format!("{pattern}: a unit code is required"));
        }
    }
    Ok(())
}

/// Splits `resource_name` into lower-case words for name suggestions.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Suggests the unit whose name or code shares the most words with the
/// resource's name or id. `None` when nothing shares a word. Returns the unit
/// code and the number of shared words; the app shows it as a suggestion
/// only, never as a mapping.
#[must_use]
pub fn suggest_unit(
    resource_name: &str,
    resource_id: &str,
    units: &[(String, String)],
) -> Option<(String, usize)> {
    let mut resource_words = words(resource_name);
    resource_words.extend(words(resource_id));
    units
        .iter()
        .map(|(code, name)| {
            let mut unit_words = words(name);
            unit_words.extend(words(code));
            let shared = unit_words
                .iter()
                .filter(|word| resource_words.contains(word))
                .count();
            (code.clone(), shared)
        })
        .filter(|(_, shared)| *shared > 0)
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(pattern: &str, unit: &str) -> AllocationRule {
        AllocationRule {
            pattern: pattern.to_owned(),
            unit_code: unit.to_owned(),
        }
    }

    #[test]
    fn exact_and_prefix_patterns() {
        assert!(pattern_matches("api-key-1", "api-key-1"));
        assert!(!pattern_matches("api-key-1", "api-key-10"));
        assert!(pattern_matches("sqeezin-*", "sqeezin-api"));
        assert!(!pattern_matches("sqeezin-*", "ledgers-api"));
    }

    #[test]
    fn first_matching_rule_wins() {
        let rules = vec![rule("ledgers-*", "LED"), rule("ledgers-api", "FZ")];
        assert_eq!(unit_for(&rules, "ledgers-api"), Some("LED"));
        assert_eq!(unit_for(&rules, "other"), None);
    }

    #[test]
    fn rules_are_validated() {
        assert!(validate_rules(&[rule("a*", "FZ")]).is_ok());
        assert!(validate_rules(&[rule("", "FZ")]).is_err());
        assert!(validate_rules(&[rule("a*b", "FZ")]).is_err());
        assert!(validate_rules(&[rule("a**", "FZ")]).is_err());
        assert!(validate_rules(&[rule("a", " ")]).is_err());
    }

    #[test]
    fn suggests_by_shared_words_only() {
        let units = vec![
            ("LED".to_owned(), "Ledgers".to_owned()),
            ("SQZ".to_owned(), "Sqeezin".to_owned()),
        ];
        assert_eq!(
            suggest_unit("sqeezin-worker", "sqeezin-worker", &units),
            Some(("SQZ".to_owned(), 1))
        );
        assert_eq!(suggest_unit("random-thing", "xyz", &units), None);
    }
}
