//! Reconciliation suggestions: which open items a bank line probably settles.
//!
//! The ranking is a pure function of its inputs, so the same bank line and
//! the same candidates always give the same list in the same order (ties
//! break on kind, then on id). Each candidate earns points from signals the
//! data states: an exact amount is the entry ticket, then the counterparty's
//! IBAN, its name, the invoice number in the remittance and the date window.
//! Text is compared, never interpreted.

use crate::rules::{LineFacts, RuleConditions};
use serde::{Deserialize, Serialize};

/// A suggestion needs at least this score to be shown.
pub const THRESHOLD: u8 = 55;
/// The most suggestions one line shows.
pub const MAX_SUGGESTIONS: usize = 3;
/// The score of a matching rule: a person wrote it for exactly this kind of line.
const RULE_SCORE: u8 = 90;
/// Days either side of a line's date that count as "close" to an open item.
const DATE_WINDOW_DAYS: i64 = 30;

/// The kind of thing a suggestion points at.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum SuggestionKind {
    /// A bank rule matched: it names the account.
    Rule,
    /// An open vendor bill the payment settles.
    Bill,
    /// A posted journal entry on the bank's ledger account.
    Entry,
    /// Multiple open bills of the same vendor settled by one bank line.
    Combined,
    /// A partial payment: one bill paid by several bank lines (or one line paying part of one bill).
    PartialPayment,
}

/// One target in a combined payment: a bill and its allocated amount.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CombinedTarget {
    /// The bill id.
    pub id: String,
    /// The amount in minor units to allocate to this bill.
    pub amount_minor: i64,
}

/// What a suggestion would do if a person accepted it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Suggestion {
    /// The kind of target.
    pub kind: SuggestionKind,
    /// The target's id: the rule, bill or entry id (or primary bill for combined/partial).
    pub target_id: String,
    /// The score, 0 to 100.
    pub score: u8,
    /// The reasons the score is what it is, as stable codes.
    pub reasons: Vec<String>,
    /// The account a rule names, when it names one.
    pub account_number: Option<String>,
    /// For combined or partial payments: the bills involved and their allocated amounts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub combined_targets: Option<Vec<CombinedTarget>>,
}

/// The bank line a suggestion is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineInput<'a> {
    /// The booking date, `YYYY-MM-DD`.
    pub booking_date: &'a str,
    /// The signed amount in minor units.
    pub minor: i64,
    /// The counterparty's name.
    pub counterparty_name: Option<&'a str>,
    /// The counterparty's IBAN, normalised.
    pub counterparty_iban: Option<&'a str>,
    /// The remittance text.
    pub remittance: &'a str,
}

/// An enabled bank rule and the account it names.
#[derive(Clone, Debug)]
pub struct RuleCandidate {
    /// The rule id.
    pub id: String,
    /// Its conditions.
    pub conditions: RuleConditions,
    /// The account the rule posts to.
    pub account_number: String,
}

/// An open vendor bill: posted, not yet settled by a bank line.
#[derive(Clone, Debug)]
pub struct BillCandidate {
    /// The bill id.
    pub id: String,
    /// The gross total in minor units (positive).
    pub gross_minor: i64,
    /// The vendor's name.
    pub vendor_name: String,
    /// The vendor's IBAN, normalised, when known.
    pub vendor_iban: Option<String>,
    /// The invoice number, when known.
    pub invoice_number: Option<String>,
    /// The invoice date, `YYYY-MM-DD`, when known.
    pub invoice_date: Option<String>,
}

/// A posted entry with a line on the bank's ledger account.
#[derive(Clone, Debug)]
pub struct EntryCandidate {
    /// The entry id.
    pub id: String,
    /// The signed amount on the bank account, from the bank's side.
    pub minor: i64,
    /// The posting date, `YYYY-MM-DD`.
    pub posting_date: String,
    /// The entry's description.
    pub description: String,
}

/// Days between two `YYYY-MM-DD` dates, or `None` when either does not parse.
#[must_use]
pub fn days_between(a: &str, b: &str) -> Option<i64> {
    fn ordinal(text: &str) -> Option<i64> {
        let year: i64 = text.get(..4)?.parse().ok()?;
        let month: i64 = text.get(5..7)?.parse().ok()?;
        let day: i64 = text.get(8..10)?.parse().ok()?;
        // Days from a fixed epoch; exact for the Gregorian calendar.
        let y = if month <= 2 { year - 1 } else { year };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let mp = (month + 9) % 12;
        let doy = (153 * mp + 2) / 5 + day - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        Some(era * 146_097 + doe)
    }
    Some((ordinal(a)? - ordinal(b)?).abs())
}

/// Lower-case letters and digits only: the form names and references are
/// compared in.
#[must_use]
pub fn compact(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Compute score points for a bill matching a bank line, based on shared signals:
/// IBAN match, name match, invoice number in remittance, and date proximity.
/// Returns (`bonus_points`, reasons). Base score (amount match) is not included here.
fn score_bill_match(
    bill: &BillCandidate,
    line: &LineInput<'_>,
    remit_compact: &str,
) -> (u8, Vec<String>) {
    let mut score: u8 = 0;
    let mut reasons = Vec::new();

    // IBAN: strong signal.
    if let (Some(iban), Some(seen)) = (&bill.vendor_iban, line.counterparty_iban)
        && iban == seen
    {
        score = score.saturating_add(30);
        reasons.push("vendor-iban".to_owned());
    } else if name_matches(&bill.vendor_name, line.counterparty_name) {
        // Name: weaker signal.
        score = score.saturating_add(15);
        reasons.push("vendor-name".to_owned());
    }

    // Invoice number in remittance.
    if let Some(number) = &bill.invoice_number {
        let needle = compact(number);
        if needle.len() >= 3 && remit_compact.contains(&needle) {
            score = score.saturating_add(25);
            reasons.push("invoice-number".to_owned());
        }
    }

    // Date window.
    if let Some(date) = &bill.invoice_date
        && days_between(date, line.booking_date).is_some_and(|d| d <= DATE_WINDOW_DAYS)
    {
        score = score.saturating_add(5);
        reasons.push("date-window".to_owned());
    }

    (score, reasons)
}

/// Ranks every suggestion for one bank line. Rules come first in the input
/// order; the output is sorted by score (highest first), then kind, then id,
/// and cut to [`MAX_SUGGESTIONS`].
#[must_use]
pub fn rank(
    line: &LineInput<'_>,
    rules: &[RuleCandidate],
    bills: &[BillCandidate],
    entries: &[EntryCandidate],
) -> Vec<Suggestion> {
    let mut out = Vec::new();
    let facts = LineFacts {
        minor: line.minor,
        counterparty_name: line.counterparty_name,
        counterparty_iban: line.counterparty_iban,
        remittance: line.remittance,
    };
    for rule in rules {
        if rule.conditions.matches(&facts) {
            out.push(Suggestion {
                kind: SuggestionKind::Rule,
                target_id: rule.id.clone(),
                score: RULE_SCORE,
                reasons: vec!["rule-matches".to_owned()],
                account_number: Some(rule.account_number.clone()),
                combined_targets: None,
            });
        }
    }
    let remit_compact = compact(line.remittance);
    for bill in bills {
        if line.minor != -bill.gross_minor {
            continue;
        }
        let (bonus, mut reasons) = score_bill_match(bill, line, &remit_compact);
        let score = 40u8.saturating_add(bonus).min(100);
        reasons.insert(0, "amount-equal".to_owned());
        if score >= THRESHOLD {
            out.push(Suggestion {
                kind: SuggestionKind::Bill,
                target_id: bill.id.clone(),
                score,
                reasons,
                account_number: None,
                combined_targets: None,
            });
        }
    }
    // Add combined payment suggestions (multiple bills, same vendor).
    if let Some(combined) = find_combined_suggestions(line, bills) {
        out.push(combined);
    }
    // Add partial payment suggestions (line pays part of one bill).
    if let Some(partial) = find_partial_suggestions(line, bills) {
        out.push(partial);
    }
    for entry in entries {
        if line.minor != entry.minor {
            continue;
        }
        let mut score: u8 = 40;
        let mut reasons = vec!["amount-equal".to_owned()];
        if days_between(&entry.posting_date, line.booking_date).is_some_and(|d| d <= 7) {
            score += 20;
            reasons.push("date-close".to_owned());
        }
        if let Some(name) = line.counterparty_name
            && compact(name).len() >= 4
            && compact(&entry.description).contains(&compact(name))
        {
            score += 20;
            reasons.push("description-names-counterparty".to_owned());
        }
        if score >= THRESHOLD {
            out.push(Suggestion {
                kind: SuggestionKind::Entry,
                target_id: entry.id.clone(),
                score: score.min(100),
                reasons,
                account_number: None,
                combined_targets: None,
            });
        }
    }
    out.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then(a.kind.cmp(&b.kind))
            .then(a.target_id.cmp(&b.target_id))
    });
    out.truncate(MAX_SUGGESTIONS);
    out
}

/// Whether a vendor's name and a counterparty's name name the same party:
/// one, after compaction, contains the other, and both are long enough.
fn name_matches(vendor: &str, counterparty: Option<&str>) -> bool {
    let Some(counterparty) = counterparty else {
        return false;
    };
    let (a, b) = (compact(vendor), compact(counterparty));
    a.len() >= 4 && b.len() >= 4 && (a.contains(&b) || b.contains(&a))
}

/// Finds combined payment suggestions: groups of bills from the same vendor
/// whose sum equals the line amount exactly (in integer minor units, same currency, correct sign).
/// Returns at most one combined suggestion with the highest score.
/// Searches bounded: at most 4 items per combination, at most 20 candidates per party.
fn find_combined_suggestions(line: &LineInput<'_>, bills: &[BillCandidate]) -> Option<Suggestion> {
    if line.minor >= 0 {
        return None;
    }

    let target_sum = -line.minor;
    let remit_compact = compact(line.remittance);

    let mut vendors: std::collections::BTreeMap<String, Vec<(usize, &BillCandidate)>> =
        std::collections::BTreeMap::new();
    for (idx, bill) in bills.iter().enumerate() {
        vendors
            .entry(bill.vendor_name.clone())
            .or_default()
            .push((idx, bill));
    }

    let mut best: Option<(u8, Vec<CombinedTarget>, Vec<String>)> = None;

    for (_vendor_name, mut vendor_bills) in vendors {
        vendor_bills.truncate(20);

        for combo_size in 2..=4.min(vendor_bills.len()) {
            let combos = combinations(&vendor_bills, combo_size);
            for combo in combos {
                let total: i64 = combo.iter().map(|(_, b)| b.gross_minor).sum();
                if total != target_sum {
                    continue;
                }

                let first_bill = &combo[0].1;
                let (bonus, mut reasons) = score_bill_match(first_bill, line, &remit_compact);
                let score = 40u8.saturating_add(bonus).min(100);
                reasons.insert(0, "combined-amount-equal".to_owned());

                if score >= THRESHOLD {
                    let targets: Vec<CombinedTarget> = combo
                        .iter()
                        .map(|(_, b)| CombinedTarget {
                            id: b.id.clone(),
                            amount_minor: b.gross_minor,
                        })
                        .collect();
                    if best.is_none() || best.as_ref().unwrap().0 < score {
                        best = Some((score, targets, reasons));
                    }
                }
            }
        }
    }

    best.map(|(score, mut targets, reasons)| {
        targets.sort_by(|a, b| a.id.cmp(&b.id));
        let target_id = targets[0].id.clone();
        Suggestion {
            kind: SuggestionKind::Combined,
            target_id,
            score: score.min(100),
            reasons,
            account_number: None,
            combined_targets: Some(targets),
        }
    })
}

/// Generates all combinations of size k from the input list, in stable order.
/// Uses iterative generation with indices to ensure deterministic output.
fn combinations<T: Clone>(items: &[(usize, T)], k: usize) -> Vec<Vec<(usize, T)>> {
    if k > items.len() {
        return Vec::new();
    }
    let mut result = Vec::new();
    let mut indices: Vec<usize> = (0..k).collect();

    loop {
        let combo: Vec<(usize, T)> = indices.iter().map(|&i| items[i].clone()).collect();
        result.push(combo);

        // Find the rightmost index to increment.
        let mut i = k - 1;
        loop {
            if indices[i] < items.len() - k + i {
                indices[i] += 1;
                for j in (i + 1)..k {
                    indices[j] = indices[j - 1] + 1;
                }
                break;
            }
            if i == 0 {
                return result;
            }
            i -= 1;
        }
    }
}

/// Finds a partial payment suggestion: one bill where the line amount is less than
/// the bill's remaining amount, with strong reference or IBAN evidence.
fn find_partial_suggestions(line: &LineInput<'_>, bills: &[BillCandidate]) -> Option<Suggestion> {
    if line.minor >= 0 {
        return None;
    }

    let line_amount = -line.minor;
    let remit_compact = compact(line.remittance);
    let mut best: Option<(u8, &BillCandidate)> = None;

    for bill in bills {
        if line_amount >= bill.gross_minor || line_amount <= 0 {
            continue;
        }

        if let (Some(iban), Some(seen)) = (&bill.vendor_iban, line.counterparty_iban) {
            if iban != seen {
                continue;
            }
        } else if !name_matches(&bill.vendor_name, line.counterparty_name) {
            continue;
        }

        let (bonus, _) = score_bill_match(bill, line, &remit_compact);
        let score = 30u8.saturating_add(bonus).min(100);

        if score >= THRESHOLD && (best.is_none() || best.as_ref().unwrap().0 < score) {
            best = Some((score, bill));
        }
    }

    best.map(|(score, bill)| Suggestion {
        kind: SuggestionKind::PartialPayment,
        target_id: bill.id.clone(),
        score,
        reasons: vec!["partial-payment".to_owned(), "vendor-iban".to_owned()],
        account_number: None,
        combined_targets: Some(vec![CombinedTarget {
            id: bill.id.clone(),
            amount_minor: line_amount,
        }]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bill(id: &str, gross: i64, iban: Option<&str>, number: Option<&str>) -> BillCandidate {
        BillCandidate {
            id: id.to_owned(),
            gross_minor: gross,
            vendor_name: "Halden Supplies GmbH".to_owned(),
            vendor_iban: iban.map(str::to_owned),
            invoice_number: number.map(str::to_owned),
            invoice_date: Some("2026-03-01".to_owned()),
        }
    }

    fn line(minor: i64, iban: Option<&'static str>, remit: &'static str) -> LineInput<'static> {
        LineInput {
            booking_date: "2026-03-05",
            minor,
            counterparty_name: Some("Halden Supplies GmbH"),
            counterparty_iban: iban,
            remittance: remit,
        }
    }

    const IBAN: &str = "DE89370400440532013000";

    #[test]
    fn combined_payment_with_invoice_references() {
        let bills = vec![
            bill("B-1", 500, Some(IBAN), Some("INV-100")),
            bill("B-2", 400, Some(IBAN), Some("INV-200")),
        ];
        let out = rank(
            &line(-900, Some(IBAN), "Zahlung INV-100 INV-200"),
            &[],
            &bills,
            &[],
        );
        let combined = out
            .iter()
            .find(|s| s.kind == SuggestionKind::Combined)
            .expect("combined");
        assert!(combined.score >= THRESHOLD);
        assert_eq!(combined.combined_targets.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn combined_payment_respects_vendor_boundary() {
        let bills = vec![
            BillCandidate {
                id: "B-1".into(),
                gross_minor: 500,
                vendor_name: "Halden Supplies GmbH".into(),
                vendor_iban: Some(IBAN.into()),
                invoice_number: Some("INV-100".into()),
                invoice_date: Some("2026-03-01".into()),
            },
            BillCandidate {
                id: "B-2".into(),
                gross_minor: 400,
                vendor_name: "Acme GmbH".into(),
                vendor_iban: None,
                invoice_number: Some("INV-200".into()),
                invoice_date: Some("2026-03-01".into()),
            },
        ];
        let out = rank(
            &line(-900, Some(IBAN), "Zahlung INV-100 INV-200"),
            &[],
            &bills,
            &[],
        );
        assert!(out.iter().all(|s| s.kind != SuggestionKind::Combined));
    }

    #[test]
    fn partial_payment_requires_strong_evidence() {
        let bills = vec![bill("B-1", 1_000, Some(IBAN), Some("INV-100"))];
        let out = rank(&line(-400, Some(IBAN), "Partial INV-100"), &[], &bills, &[]);
        let partial = out
            .iter()
            .find(|s| s.kind == SuggestionKind::PartialPayment);
        assert!(partial.is_some());

        let bills = vec![BillCandidate {
            id: "B-1".into(),
            gross_minor: 1_000,
            vendor_name: "Unknown".into(),
            vendor_iban: Some("DE123456789".into()),
            invoice_number: Some("INV-100".into()),
            invoice_date: Some("2026-03-01".into()),
        }];
        let out = rank(&line(-400, Some(IBAN), "Transfer"), &[], &bills, &[]);
        let partial = out
            .iter()
            .find(|s| s.kind == SuggestionKind::PartialPayment);
        assert!(partial.is_none());
    }

    #[test]
    fn suggestions_deterministic_with_shuffled_input() {
        let bills = vec![
            bill("B-1", 300, Some(IBAN), Some("INV-1")),
            bill("B-2", 300, Some(IBAN), Some("INV-2")),
        ];
        let line_input = line(-600, Some(IBAN), "Zahlung INV-1 INV-2");
        let out1 = rank(&line_input, &[], &bills, &[]);

        let bills_shuffled = vec![
            bill("B-2", 300, Some(IBAN), Some("INV-2")),
            bill("B-1", 300, Some(IBAN), Some("INV-1")),
        ];
        let out2 = rank(&line_input, &[], &bills_shuffled, &[]);
        assert_eq!(out1, out2);
    }
}
