//! Matching logic: 2-way and 3-way proposals with confidence scoring.

use crate::model::{GoodsReceipt, MatchBill, PurchaseOrder, Quantity};
use crate::{MatchError, Tolerances};
use ledgers_money::Amount;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::BTreeMap;

/// Format an Amount with a leading '+' when positive (canonical string carries '-' when negative).
fn signed(a: Amount) -> String {
    let s = a.to_canonical_string();
    if a.minor() > 0 { format!("+{s}") } else { s }
}

/// Auto-accept confidence threshold.
pub const AUTO_ACCEPT_CONFIDENCE: u8 = 90;

/// Normalize PO reference: uppercase, ASCII alphanumerics only.
#[must_use]
pub fn normalize_reference(s: &str) -> String {
    s.to_uppercase()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect()
}

/// Normalize vendor: trim, lowercase, collapse whitespace.
#[must_use]
pub fn normalize_vendor(s: &str) -> String {
    s.trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// 2-way (PO + invoice) or 3-way (PO + receipts + invoice).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchKind {
    TwoWay,
    ThreeWay,
}

/// Explanation: matched, differs (within tolerance), and blockers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Explanation {
    pub matched: Vec<String>,
    pub differs: Vec<String>,
    pub blockers: Vec<String>,
}

/// Proposal: matching result ready for review or auto-acceptance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    pub kind: MatchKind,
    pub bill: String,
    pub po: String,
    pub receipts: Vec<String>,
    pub confidence: u8,
    pub auto_accept: bool,
    pub explanation: Explanation,
}

/// Propose matching: one proposal per matching PO, sorted by confidence desc then PO number asc.
///
/// # Errors
///
/// Returns error if no POs match the bill's vendor and currency.
///
/// # Panics
///
/// Never panics; an `expect()` is used only when a PO line is guaranteed to exist in `bill_line_matches`.
#[allow(
    clippy::too_many_lines,
    clippy::comparison_chain,
    clippy::cast_possible_truncation
)]
pub fn propose(
    bill: &MatchBill,
    pos: &[PurchaseOrder],
    receipts: &[GoodsReceipt],
    tolerances: &Tolerances,
) -> Result<Vec<Proposal>, MatchError> {
    // Filter candidates: same normalized vendor and same currency.
    let bill_vendor_norm = normalize_vendor(&bill.vendor);
    let candidates: Vec<_> = pos
        .iter()
        .filter(|po| {
            normalize_vendor(&po.vendor) == bill_vendor_norm && po.currency == bill.currency
        })
        .collect();

    if candidates.is_empty() {
        return Err(MatchError::NoPosFound);
    }

    let mut proposals = Vec::new();

    for po in candidates {
        let po_vendor_norm = normalize_vendor(&po.vendor);
        let receipt_map = build_receipt_map(receipts, &po.number, &po_vendor_norm);
        let has_receipts = !receipt_map.is_empty();
        let mut bill_line_matches: BTreeMap<usize, u32> = BTreeMap::new();
        let mut matched_line_qty_ok: u8 = 0;
        let mut matched_line_price_ok: u8 = 0;
        let mut explanation = Explanation {
            matched: vec![],
            differs: vec![],
            blockers: vec![],
        };

        for (bill_idx, bill_line) in bill.lines.iter().enumerate() {
            let mut best_po_line = None;
            if let Some(bill_item) = &bill_line.item {
                for po_line in &po.lines {
                    if po_line.item == *bill_item {
                        best_po_line = Some(po_line.line);
                        break;
                    }
                }
            }
            if best_po_line.is_none() {
                let bill_desc = bill_line.description.trim().to_uppercase();
                for po_line in &po.lines {
                    if po_line.description.trim().to_uppercase() == bill_desc {
                        best_po_line = Some(po_line.line);
                        break;
                    }
                }
            }

            if let Some(po_line_num) = best_po_line {
                bill_line_matches.insert(bill_idx, po_line_num);
            } else {
                // Unmatched bill line is a blocker.
                explanation.blockers.push(format!(
                    "bill line {} {}: no PO line with this item or description",
                    bill_idx + 1,
                    bill_line.description
                ));
            }
        }

        let mut po_ref_matches = false;
        if let Some(bill_ref) = &bill.po_reference {
            po_ref_matches = normalize_reference(bill_ref) == normalize_reference(&po.number);
        }
        let tolerance = tolerances.for_vendor(&po.vendor);
        let mut cum_billed_by_po: std::collections::BTreeMap<u32, Quantity> =
            std::collections::BTreeMap::new();

        for (bill_idx, po_line_num) in &bill_line_matches {
            let bill_line = &bill.lines[*bill_idx];
            let po_line = po
                .lines
                .iter()
                .find(|l| l.line == *po_line_num)
                .expect("po_line must exist");

            // Compute available quantity (open on order or received).
            let available = if has_receipts {
                // 3-way: received - billed
                receipt_map.get(po_line_num).copied().unwrap_or_default()
            } else {
                // 2-way: ordered - billed
                po_line.quantity
            };

            let cum = cum_billed_by_po
                .get(po_line_num)
                .copied()
                .unwrap_or_default();
            let avail_final = Quantity::from_thousandths(
                available.to_thousandths() - po_line.billed.to_thousandths() - cum.to_thousandths(),
            );
            let basis = if has_receipts {
                "received"
            } else {
                "open on order"
            };
            let qty_variance = Quantity::from_thousandths(
                bill_line.quantity.to_thousandths() - avail_final.to_thousandths(),
            );

            if qty_variance.to_thousandths() > 0 {
                // Over-billed.
                if tolerance.is_quantity_within(qty_variance, avail_final) {
                    explanation.differs.push(format!(
                        "line {} {}: billed {} {} of {} {}, within tolerance",
                        po_line_num,
                        po_line.description,
                        bill_line.quantity,
                        if po_line.unit_price.currency().code() == "EUR" {
                            "EUR"
                        } else {
                            "unit"
                        },
                        avail_final,
                        basis
                    ));
                } else {
                    explanation.blockers.push(format!(
                        "line {} {}: billed {} exceeds {} {} by {}, tolerance {}",
                        po_line_num,
                        po_line.description,
                        bill_line.quantity,
                        avail_final,
                        basis,
                        qty_variance,
                        tolerance.quantity_abs
                    ));
                }
            } else if qty_variance.to_thousandths() < 0 {
                // Under-billed (partial delivery) — not a blocker.
                explanation.differs.push(format!(
                    "line {} {}: quantity {} within {} {}",
                    po_line_num, po_line.description, bill_line.quantity, avail_final, basis
                ));
            } else {
                // Exact match.
                matched_line_qty_ok += 1;
                explanation.matched.push(format!(
                    "line {} {}: quantity {} within {} {}",
                    po_line_num, po_line.description, bill_line.quantity, avail_final, basis
                ));
            }

            // Price check: compute variance and determine if price-OK.
            let currency = po_line.unit_price.currency();
            let unit_diff = bill_line.unit_price.checked_sub(po_line.unit_price)?;
            let (b, p, c) = (
                bill_line.unit_price.to_canonical_string(),
                po_line.unit_price.to_canonical_string(),
                currency.code(),
            );

            match unit_diff.minor().cmp(&0) {
                Ordering::Equal => {
                    matched_line_price_ok += 1;
                    explanation.matched.push(format!(
                        "line {} {}: price {} {}/unit as ordered",
                        po_line_num, po_line.description, p, c
                    ));
                }
                Ordering::Less => {
                    // Billed cheaper than PO (always price-OK, not a blocker).
                    let total_diff =
                        crate::model::line_amount(bill_line.unit_price, bill_line.quantity)?
                            .checked_sub(crate::model::line_amount(
                                po_line.unit_price,
                                bill_line.quantity,
                            )?)?;
                    matched_line_price_ok += 1;
                    explanation.differs.push(format!(
                        "line {} {}: billed {} {}/unit vs PO {} {}/unit, {} per unit, {} {} over {} units",
                        po_line_num, po_line.description, b, c, p, c,
                        signed(unit_diff), signed(total_diff), c, bill_line.quantity,
                    ));
                }
                Ordering::Greater => {
                    // Billed higher than PO: check tolerance.
                    let ordered =
                        crate::model::line_amount(po_line.unit_price, bill_line.quantity)?;
                    let total_diff =
                        crate::model::line_amount(bill_line.unit_price, bill_line.quantity)?
                            .checked_sub(ordered)?;
                    let allowed = Amount::new(tolerance.allowed_price(ordered.minor()), currency);
                    let detail = format!(
                        "line {} {}: billed {} {}/unit vs PO {} {}/unit, {} per unit, {} {} over {} units",
                        po_line_num,
                        po_line.description,
                        b,
                        c,
                        p,
                        c,
                        signed(unit_diff),
                        signed(total_diff),
                        c,
                        bill_line.quantity,
                    );
                    if total_diff.minor() <= allowed.minor() {
                        matched_line_price_ok += 1;
                        explanation.differs.push(format!(
                            "{}, within tolerance {} {}",
                            detail,
                            allowed.to_canonical_string(),
                            c
                        ));
                    } else {
                        explanation.blockers.push(format!(
                            "{}, tolerance {} {}",
                            detail,
                            allowed.to_canonical_string(),
                            c
                        ));
                    }
                }
            }

            // Accumulate this bill line's quantity against the PO line.
            cum_billed_by_po
                .entry(*po_line_num)
                .and_modify(|q| {
                    *q = Quantity::from_thousandths(
                        q.to_thousandths() + bill_line.quantity.to_thousandths(),
                    );
                })
                .or_insert(bill_line.quantity);
        }

        // Compute confidence: 30 (vendor+currency) + 20 (if PO ref matches) +
        // 30×matched_lines/n + 10×quantity_ok/n + 10×price_ok/n (integer floor),
        // where n = max(1, bill.lines.len()).
        let n = bill.lines.len().max(1);
        let score = 30
            + if po_ref_matches { 20 } else { 0 }
            + 30 * bill_line_matches.len() / n
            + 10 * usize::from(matched_line_qty_ok) / n
            + 10 * usize::from(matched_line_price_ok) / n;
        let confidence = u8::try_from(score.min(100)).unwrap_or(100);

        // Add header explanations.
        explanation
            .matched
            .insert(0, format!("currency {}", bill.currency.code()));
        explanation
            .matched
            .insert(0, format!("vendor {}", po.vendor));

        if bill.po_reference.is_some() {
            if po_ref_matches {
                explanation
                    .matched
                    .push(format!("PO reference {}", po.number));
            } else {
                explanation
                    .differs
                    .push("no PO reference on the bill".to_string());
            }
        }

        // Determine 2-way vs 3-way.
        let kind = if has_receipts {
            MatchKind::ThreeWay
        } else {
            MatchKind::TwoWay
        };

        // Build receipts list.
        let mut receipts_for_proposal: Vec<String> = receipts
            .iter()
            .filter(|r| r.po_number == po.number && normalize_vendor(&r.vendor) == bill_vendor_norm)
            .map(|r| r.number.clone())
            .collect();
        receipts_for_proposal.sort();
        receipts_for_proposal.dedup();

        let auto_accept = explanation.blockers.is_empty() && confidence >= AUTO_ACCEPT_CONFIDENCE;

        proposals.push(Proposal {
            kind,
            bill: bill.number.clone(),
            po: po.number.clone(),
            receipts: receipts_for_proposal,
            confidence,
            auto_accept,
            explanation,
        });
    }

    // Sort by confidence descending, then PO number ascending.
    proposals.sort_by(|a, b| {
        b.confidence
            .cmp(&a.confidence)
            .then_with(|| a.po.cmp(&b.po))
    });

    Ok(proposals)
}

/// Build a map of `po_line` -> sum of received quantity from all receipts for a PO.
///
/// A receipt counts only when both its PO number and normalized vendor match.
fn build_receipt_map(
    receipts: &[GoodsReceipt],
    po_number: &str,
    po_vendor_norm: &str,
) -> BTreeMap<u32, Quantity> {
    let mut map: BTreeMap<u32, Quantity> = BTreeMap::new();

    for receipt in receipts {
        if receipt.po_number == po_number && normalize_vendor(&receipt.vendor) == po_vendor_norm {
            for line in &receipt.lines {
                map.entry(line.po_line)
                    .and_modify(|q| {
                        *q = Quantity::from_thousandths(
                            q.to_thousandths() + line.quantity.to_thousandths(),
                        );
                    })
                    .or_insert(line.quantity);
            }
        }
    }

    map
}
