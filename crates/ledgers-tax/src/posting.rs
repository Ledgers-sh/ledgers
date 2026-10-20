//! Convert tax calculations to [`ledgers_ledger::Line`]s.

use crate::calc::TaxLine;
use crate::code::TaxKind;
use crate::error::TaxError;
use ledgers_ledger::{Line, Side};
use ledgers_money::{Amount, Rate, Rounding};
use std::collections::BTreeMap;

/// Convert a tax line into ledger lines for posting.
///
/// # Errors
///
/// Returns [`TaxError`] if calculation fails (e.g., overflow during rounding).
pub fn tax_lines_from_calculated(
    tax_line: &TaxLine,
    account_number: &str,
    tax_kind: TaxKind,
    output_vat_account: Option<&str>,
    input_vat_account: Option<&str>,
    non_deductible_share: Rate,
    _currency: ledgers_money::Currency,
) -> Result<Vec<Line>, TaxError> {
    let mut lines = Vec::new();

    // Helper to handle input VAT posting with non-deductible split
    let add_input_vat = |lines: &mut Vec<Line>, tax: Amount| -> Result<(), TaxError> {
        if tax.is_zero() {
            return Ok(());
        }
        if let Some(acct) = input_vat_account {
            let non_ded = tax.mul_rate(non_deductible_share, Rounding::HalfEven)?;
            let ded = tax.checked_sub(non_ded)?;
            if !ded.is_zero() {
                lines.push(Line {
                    account_number: acct.to_string(),
                    side: Side::Debit,
                    base: ded,
                    fx: None,
                    tax_code: None,
                    dimensions: BTreeMap::new(),
                    memo: None,
                });
            }
            if !non_ded.is_zero() {
                lines.push(Line {
                    account_number: account_number.to_string(),
                    side: Side::Debit,
                    base: non_ded,
                    fx: None,
                    tax_code: None,
                    dimensions: BTreeMap::new(),
                    memo: None,
                });
            }
        }
        Ok(())
    };

    match tax_kind {
        TaxKind::OutputVat => {
            if !tax_line.output_vat.is_zero()
                && let Some(acct) = output_vat_account
            {
                lines.push(Line {
                    account_number: acct.to_string(),
                    side: Side::Credit,
                    base: tax_line.output_vat,
                    fx: None,
                    tax_code: None,
                    dimensions: BTreeMap::new(),
                    memo: None,
                });
            }
        }
        TaxKind::InputVat | TaxKind::ImportVat => {
            add_input_vat(&mut lines, tax_line.input_vat)?;
        }
        TaxKind::ReverseCharge => {
            if !tax_line.output_vat.is_zero()
                && let Some(acct) = output_vat_account
            {
                lines.push(Line {
                    account_number: acct.to_string(),
                    side: Side::Credit,
                    base: tax_line.output_vat,
                    fx: None,
                    tax_code: None,
                    dimensions: BTreeMap::new(),
                    memo: None,
                });
            }
            add_input_vat(&mut lines, tax_line.input_vat)?;
        }
        TaxKind::Exempt | TaxKind::Zero => {}
    }

    Ok(lines)
}
