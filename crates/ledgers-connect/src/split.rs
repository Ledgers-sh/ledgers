//! The split of one invoice across business units, and the bill lines it
//! becomes (issue #142). Used by the import and by re-splitting when an
//! allocation rule or the default unit changes, so both always agree.

use crate::allocation::{AllocationRule, unit_for};
use crate::money::split_by_weight;
use crate::store::UsageRow;
use ledgers_bills::model::BillLine;
use std::collections::BTreeMap;

/// The result of splitting one invoice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Split {
    /// `(unit code, minor units)`, sorted by unit code; sums to the total.
    pub parts: Vec<(String, i64)>,
    /// Resources with no rule, as `kind:id`, that booked to the default unit.
    pub unmapped: Vec<String>,
    /// True when the period had no usage, so the whole amount booked to the default unit.
    pub no_usage: bool,
}

/// Splits `total` for a period whose usage is `usage` by the rules.
///
/// The weight of a unit is the sum of the quantities of its resources.
/// Resources with no rule book to `default_unit` and are reported in
/// `unmapped`. A period with no usage books the whole total to the default
/// unit and sets `no_usage`.
#[must_use]
pub fn split(
    total: i64,
    usage: &[UsageRow],
    rules: &[AllocationRule],
    default_unit: &str,
) -> Split {
    let mut weights: BTreeMap<String, u128> = BTreeMap::new();
    let mut unmapped = Vec::new();
    for row in usage {
        if row.quantity <= 0 {
            continue;
        }
        let unit = if let Some(unit) = unit_for(rules, &row.resource_id) {
            unit.to_owned()
        } else {
            let label = format!("{}:{}", row.resource_kind, row.resource_id);
            if !unmapped.contains(&label) {
                unmapped.push(label);
            }
            default_unit.to_owned()
        };
        let quantity = u128::try_from(row.quantity).unwrap_or(0);
        *weights.entry(unit).or_insert(0) += quantity;
    }
    let weight_list: Vec<(String, u128)> = weights.into_iter().collect();
    match split_by_weight(total, &weight_list) {
        Some(parts) => Split {
            parts,
            unmapped,
            no_usage: false,
        },
        None => Split {
            parts: vec![(default_unit.to_owned(), total)],
            unmapped,
            no_usage: true,
        },
    }
}

/// The net amount of a gross total at `vat_rate_bp` (basis points): the gross
/// divided by `1 + rate`, rounded half up once. A rate of zero means no VAT,
/// so the net is the gross. The vendor's amount is a gross total; the VAT
/// split is derived here, exactly, from the rate the connection configures.
#[must_use]
pub fn net_of_gross(gross: i64, vat_rate_bp: i64) -> i64 {
    if vat_rate_bp <= 0 {
        return gross;
    }
    let numerator = i128::from(gross) * 10_000;
    let denominator = 10_000 + i128::from(vat_rate_bp);
    let rounded = (numerator * 2 + denominator).div_euclid(denominator * 2);
    i64::try_from(rounded).unwrap_or(gross)
}

/// The bill lines of a split: one per unit, carrying its share of the net
/// (the bill's VAT is gross less net), its unit and the expense account.
#[must_use]
pub fn bill_lines(
    parts: &[(String, i64)],
    description: &str,
    expense_account: &str,
    vat_rate_bp: Option<i64>,
) -> Vec<BillLine> {
    parts
        .iter()
        .map(|(unit, net)| BillLine {
            description: format!("{description} ({unit})"),
            net_minor: Some(*net),
            vat_rate_bp,
            vat_minor: None,
            gross_minor: None,
            expense_account: Some(expense_account.to_owned()),
            suggested_from: None,
            unit_code: Some(unit.clone()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(id: &str, quantity: i64) -> UsageRow {
        UsageRow {
            period: "2026-08".to_owned(),
            resource_kind: "worker".to_owned(),
            resource_id: id.to_owned(),
            resource_name: id.to_owned(),
            quantity,
        }
    }

    fn rule(pattern: &str, unit: &str) -> AllocationRule {
        AllocationRule {
            pattern: pattern.to_owned(),
            unit_code: unit.to_owned(),
        }
    }

    #[test]
    fn split_by_mapped_usage_sums_to_the_invoice() {
        let rules = vec![rule("ledgers-*", "LED"), rule("sqz-api", "SQZ")];
        let usage = vec![
            usage("ledgers-web", 700),
            usage("ledgers-api", 200),
            usage("sqz-api", 100),
        ];
        let result = split(12_345, &usage, &rules, "FZ");
        let sum: i64 = result.parts.iter().map(|(_, amount)| amount).sum();
        assert_eq!(sum, 12_345);
        assert_eq!(result.parts.len(), 2);
        assert!(result.unmapped.is_empty());
        assert!(!result.no_usage);
    }

    #[test]
    fn unmapped_resources_book_to_the_default_and_are_reported() {
        let rules = vec![rule("ledgers-*", "LED")];
        let usage = vec![usage("ledgers-web", 100), usage("mystery", 100)];
        let result = split(1_000, &usage, &rules, "FZ");
        assert_eq!(
            result.parts,
            vec![("FZ".to_owned(), 500), ("LED".to_owned(), 500)]
        );
        assert_eq!(result.unmapped, vec!["worker:mystery".to_owned()]);
    }

    #[test]
    fn no_usage_books_everything_to_the_default_unit() {
        let result = split(777, &[], &[], "FZ");
        assert_eq!(result.parts, vec![("FZ".to_owned(), 777)]);
        assert!(result.no_usage);
    }

    #[test]
    fn zero_usage_counts_as_no_usage() {
        let result = split(777, &[usage("a", 0)], &[], "FZ");
        assert!(result.no_usage);
        assert_eq!(result.parts, vec![("FZ".to_owned(), 777)]);
    }

    #[test]
    fn bill_lines_carry_their_unit_and_sum_to_the_net() {
        let lines = bill_lines(
            &[("FZ".to_owned(), 300), ("LED".to_owned(), 700)],
            "Cloudflare",
            "6100",
            Some(2100),
        );
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].unit_code.as_deref(), Some("FZ"));
        let sum: i64 = lines.iter().filter_map(|line| line.net_minor).sum();
        assert_eq!(sum, 1_000);
    }

    #[test]
    fn net_is_gross_over_one_plus_rate_rounded_once() {
        assert_eq!(net_of_gross(12_100, 2_100), 10_000);
        assert_eq!(net_of_gross(1_000, 0), 1_000);
        assert_eq!(net_of_gross(1_000, 1_900), 840);
        // 1000 / 1.19 = 840.336...: rounds to 840; 1500 / 1.19 = 1260.504...: to 1261.
        assert_eq!(net_of_gross(1_500, 1_900), 1_261);
    }
}
