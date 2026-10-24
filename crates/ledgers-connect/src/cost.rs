//! The shape cost-metered vendors share (issue #142 cloud connectors).
//!
//! A cloud vendor reports cost per resource (a project, a service, a tag
//! value). The books need one invoice per vendor, period and currency, and the
//! allocation needs each resource's share. This module sums the vendor's
//! amounts exactly, at six decimals, and turns them into:
//!
//! - one [`InvoiceRecord`] per `(period, currency)` whose total is the sum of
//!   the resource costs, with external id `{vendor}-cost:{period}:{currency}`
//!   (re-syncing the same period is a no-op in the import), and
//! - one [`UsageRecord`] per resource and period whose `quantity` is the
//!   resource's cost in minor units of the currency. For these vendors the
//!   usage weight is the cost itself, so the proportional split books each
//!   resource exactly its share of the invoice.
//!
//! Only periods that have ended before `today` are booked, because a cost is
//! still moving until its month closes.

use crate::http::sanitize_vendor_text;
use crate::model::{InvoiceRecord, SyncOutput, UsageRecord};
use crate::money::{AmountError, parse_decimal};
use ledgers_money::Currency;
use std::collections::BTreeMap;

/// The decimals a vendor amount is summed at, before rounding to the currency.
pub const MICRO_SCALE: u32 = 6;

/// One vendor amount, attributed to a resource in one period.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CostLine {
    /// `YYYY-MM`.
    pub period: String,
    /// ISO 4217 code, upper case.
    pub currency: String,
    /// What kind of resource: `project`, `service`, `tag`, `organization`.
    pub kind: &'static str,
    /// The vendor's id for the resource (the allocation key).
    pub id: String,
    /// A human name for the resource.
    pub name: String,
    /// The amount in millionths of the currency unit.
    pub micro: i128,
}

/// Parses a vendor amount at [`MICRO_SCALE`].
///
/// # Errors
/// [`AmountError`] for anything that is not a plain decimal.
pub fn micro_of(text: &str) -> Result<i128, AmountError> {
    parse_decimal(text, MICRO_SCALE)
}

/// Rounds `micro` (millionths) half up to the currency's minor units.
#[must_use]
pub fn to_minor(micro: i128, exponent: u32) -> i128 {
    if exponent >= MICRO_SCALE {
        let factor = 10_i128.pow(exponent - MICRO_SCALE);
        return micro * factor;
    }
    let divisor = 10_i128.pow(MICRO_SCALE - exponent);
    let half = divisor / 2;
    if micro >= 0 {
        (micro + half) / divisor
    } else {
        -((-micro + half) / divisor)
    }
}

/// The month `YYYY-MM` of a `YYYY-MM-DD` or RFC 3339 text, or `None`.
#[must_use]
pub fn month_of(text: &str) -> Option<String> {
    let head = text.get(0..7)?;
    let bytes = head.as_bytes();
    let valid = bytes[4] == b'-'
        && head[..4].bytes().all(|b| b.is_ascii_digit())
        && head[5..].bytes().all(|b| b.is_ascii_digit());
    let month: u32 = head.get(5..7)?.parse().ok()?;
    (valid && (1..=12).contains(&month)).then(|| head.to_owned())
}

/// Builds the sync output for a set of cost lines.
///
/// `vendor` is the connector id (`aws`), `vendor_name` the name the bill shows,
/// `today` the `YYYY-MM-DD` of the sync. Periods that have not ended by `today`
/// are skipped with a note. A currency Ledgers does not know is skipped with a
/// note, never booked in a guessed unit.
#[must_use]
pub fn output(
    vendor: &str,
    vendor_name: &str,
    lines: &[CostLine],
    today: &str,
    notes: &mut Vec<String>,
) -> SyncOutput {
    let current = month_of(today).unwrap_or_default();
    let mut totals: BTreeMap<(String, String), i128> = BTreeMap::new();
    let mut resources: BTreeMap<(String, String, String, String), (i128, String)> = BTreeMap::new();
    let mut unknown_currency: Vec<String> = Vec::new();
    for line in lines {
        if line.period >= current {
            continue;
        }
        if Currency::from_code(&line.currency).is_err() {
            if !unknown_currency.contains(&line.currency) {
                unknown_currency.push(line.currency.clone());
            }
            continue;
        }
        *totals
            .entry((line.period.clone(), line.currency.clone()))
            .or_insert(0) += line.micro;
        let key = (
            line.period.clone(),
            line.currency.clone(),
            line.kind.to_owned(),
            line.id.clone(),
        );
        let entry = resources.entry(key).or_insert((0, line.name.clone()));
        entry.0 += line.micro;
    }
    if !unknown_currency.is_empty() {
        notes.push(format!(
            "amounts in {} are not booked: Ledgers does not know the currency",
            unknown_currency.join(", ")
        ));
    }
    let mut out = SyncOutput::default();
    for ((period, currency), micro) in &totals {
        let exponent = exponent_of(currency);
        let total = to_minor(*micro, exponent);
        if total <= 0 {
            if total < 0 {
                notes.push(format!(
                    "{vendor} net cost for {period} in {currency} is negative and is not booked"
                ));
            }
            continue;
        }
        let Ok(total_minor) = i64::try_from(total) else {
            notes.push(format!(
                "{vendor} total for {period} is out of range and is skipped"
            ));
            continue;
        };
        out.invoices.push(InvoiceRecord {
            external_id: format!("{vendor}-cost:{period}:{currency}"),
            vendor_name: vendor_name.to_owned(),
            invoice_number: None,
            issued_on: format!("{period}-01"),
            period: period.clone(),
            currency: currency.clone(),
            total_minor,
            description: sanitize_vendor_text(&format!("{vendor_name} usage {period}")),
            hosted_url: None,
        });
    }
    for ((period, currency, kind, id), (micro, name)) in resources {
        if !totals.contains_key(&(period.clone(), currency.clone())) {
            continue;
        }
        let quantity = to_minor(micro, exponent_of(&currency));
        out.usage.push(UsageRecord {
            period,
            resource_kind: kind,
            resource_id: id.clone(),
            resource_name: sanitize_vendor_text(if name.is_empty() { &id } else { &name }),
            quantity: i64::try_from(quantity.max(0)).unwrap_or(i64::MAX),
        });
    }
    out
}

fn exponent_of(code: &str) -> u32 {
    Currency::from_code(code).map_or(2, ledgers_money::Currency::exponent)
}

/// Seconds since the Unix epoch for an RFC 3339 UTC instant
/// (`2026-10-24T10:00:00Z`), or `None`.
#[must_use]
pub fn epoch_seconds(now: &str) -> Option<i64> {
    let year: i64 = now.get(0..4)?.parse().ok()?;
    let month: i64 = now.get(5..7)?.parse().ok()?;
    let day: i64 = now.get(8..10)?.parse().ok()?;
    let hour: i64 = now.get(11..13)?.parse().ok()?;
    let minute: i64 = now.get(14..16)?.parse().ok()?;
    let second: i64 = now.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Days from the civil date (Howard Hinnant's algorithm), proleptic Gregorian.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(period: &str, currency: &str, id: &str, micro: i128) -> CostLine {
        CostLine {
            period: period.to_owned(),
            currency: currency.to_owned(),
            kind: "service",
            id: id.to_owned(),
            name: id.to_owned(),
            micro,
        }
    }

    #[test]
    fn rounds_half_up_to_the_currency() {
        assert_eq!(to_minor(1_234_567, 2), 123);
        assert_eq!(to_minor(1_235_000, 2), 124);
        assert_eq!(to_minor(-1_235_000, 2), -124);
        assert_eq!(to_minor(1_500_000, 0), 2);
        assert_eq!(to_minor(500_000, 3), 500);
    }

    #[test]
    fn one_invoice_per_period_and_currency_with_resource_weights() {
        let lines = vec![
            line("2026-07", "USD", "ledgers", 10_000_000),
            line("2026-07", "USD", "sqeezin", 30_000_000),
            line("2026-07", "EUR", "ledgers", 5_000_000),
        ];
        let mut notes = Vec::new();
        let out = output(
            "aws",
            "Amazon Web Services",
            &lines,
            "2026-10-24",
            &mut notes,
        );
        let mut ids: Vec<&str> = out
            .invoices
            .iter()
            .map(|i| i.external_id.as_str())
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, ["aws-cost:2026-07:EUR", "aws-cost:2026-07:USD"]);
        let usd = out
            .invoices
            .iter()
            .find(|i| i.currency == "USD")
            .expect("USD invoice");
        assert_eq!(usd.total_minor, 4000);
        assert_eq!(out.usage.len(), 3);
        assert!(notes.is_empty());
    }

    #[test]
    fn periods_that_have_not_ended_and_unknown_currencies_are_not_booked() {
        let lines = vec![
            line("2026-10", "USD", "a", 1_000_000),
            line("2026-09", "XXZ", "a", 1_000_000),
            line("2026-09", "USD", "a", 1_000_000),
        ];
        let mut notes = Vec::new();
        let out = output("aws", "AWS", &lines, "2026-10-24", &mut notes);
        assert_eq!(out.invoices.len(), 1);
        assert_eq!(out.invoices[0].period, "2026-09");
        assert!(notes.iter().any(|n| n.contains("XXZ")));
    }

    #[test]
    fn epoch_seconds_match_known_instants() {
        assert_eq!(epoch_seconds("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch_seconds("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(epoch_seconds("2026-10-24T10:00:00Z"), Some(1_792_836_000));
        assert_eq!(epoch_seconds("nope"), None);
    }

    #[test]
    fn micro_parsing_is_exact() {
        assert_eq!(micro_of("12.5").ok(), Some(12_500_000));
        assert!(micro_of("1e-3").is_err());
    }
}
