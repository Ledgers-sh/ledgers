//! Tax line calculation: net-based and gross-based with per-line or
//! per-document rounding.

use crate::code::{TaxCode, TaxKind};
use crate::error::TaxError;
use crate::rate::RateTable;
use ledgers_money::{Amount, Currency, Rate, Rounding};
use std::collections::BTreeMap;
use time::Date;

/// The basis for tax calculation: the document's amounts are expressed
/// in this basis, and tax is computed accordingly.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Basis {
    /// Amounts are net; tax is computed as `net × rate` rounded.
    Net,
    /// Amounts are gross; tax is computed as `gross × rate / (1 + rate)` rounded,
    /// so `net + tax == gross` exactly.
    Gross,
}

/// The scope at which rounding is applied for per-document calculations.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum RoundingScope {
    /// Round each line's tax independently.
    PerLine,
    /// Group lines by rate, compute tax once per group, distribute to lines
    /// using largest-remainder allocation so the total is exact.
    PerDocument,
}

/// One line of input to the tax calculator.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TaxLineInput {
    /// The gross or net amount (depending on [`Basis`]).
    pub amount: Amount,
    /// The tax code (looked up in the registry).
    pub tax_code: String,
}

/// One calculated tax line in the output.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TaxLine {
    /// The line's net amount.
    pub net: Amount,
    /// The tax (output, input, or both if reverse charge).
    pub output_vat: Amount,
    /// Input VAT (reverse charge or import VAT).
    pub input_vat: Amount,
    /// The gross amount (`net + max(output_vat, input_vat)`).
    pub gross: Amount,
}

/// The complete tax calculation result.
#[derive(Clone, Debug)]
pub struct TaxCalculation {
    /// The calculated lines.
    pub lines: Vec<(TaxLineInput, TaxLine)>,
    /// Total net across all lines.
    pub total_net: Amount,
    /// Total output VAT across all lines.
    pub total_output_vat: Amount,
    /// Total input VAT across all lines.
    pub total_input_vat: Amount,
    /// Total gross.
    pub total_gross: Amount,
    /// When the company is a small-business exemption (Kleinunternehmer),
    /// the required invoice note (e.g., "Gemäß § 19 `UStG` wird keine
    /// Umsatzsteuer berechnet.").
    pub small_business_note: Option<String>,
}

/// Parameters for [`calculate`].
#[derive(Clone)]
pub struct CalculateParams<'a> {
    /// The document lines (amount, tax code).
    pub lines: &'a [TaxLineInput],
    /// The document date; used to look up rates in the table.
    pub date: Date,
    /// The rate table (e.g., from [`crate::rate::germany_rates`]).
    pub table: &'a RateTable,
    /// Function to look up a [`TaxCode`] by string.
    pub code_lookup: &'a dyn Fn(&str) -> Result<TaxCode, TaxError>,
    /// Whether amounts are net or gross.
    pub basis: Basis,
    /// Per-line or per-document rounding.
    pub scope: RoundingScope,
    /// The rounding mode.
    pub rounding: Rounding,
    /// If true, tax is zero on output VAT and the required note is returned.
    pub small_business: bool,
}

/// Calculate tax for a list of lines.
///
/// # Errors
///
/// Returns [`TaxError::*`] for missing codes, rates, or calculation errors.
pub fn calculate(params: &CalculateParams) -> Result<TaxCalculation, TaxError> {
    let lines = params.lines;
    let date = params.date;
    let table = params.table;
    let code_lookup = params.code_lookup;
    let basis = params.basis;
    let scope = params.scope;
    let rounding = params.rounding;
    let small_business = params.small_business;
    if lines.is_empty() {
        return Err(TaxError::Invalid {
            reason: "no lines to calculate".to_string(),
        });
    }

    let currency = lines[0].amount.currency();
    for line in lines {
        if line.amount.currency() != currency {
            return Err(TaxError::Money(
                ledgers_money::MoneyError::CurrencyMismatch {
                    expected: currency,
                    actual: line.amount.currency(),
                },
            ));
        }
    }

    // Look up all codes and rates upfront.
    let mut codes = Vec::new();
    let mut rates = Vec::new();
    for line in lines {
        let code = code_lookup(&line.tax_code)?;
        let rate = table.rate_on(crate::rate::Jurisdiction::Germany, code.category, date)?;
        codes.push(code);
        rates.push(rate.rate);
    }

    // Calculate per line if PerLine scope; otherwise prepare for per-document grouping.
    let calculated_lines: Vec<(TaxLineInput, TaxLine)> = if scope == RoundingScope::PerLine {
        lines
            .iter()
            .zip(&codes)
            .zip(&rates)
            .map(|((input, code), rate)| {
                calculate_line(
                    input,
                    code,
                    *rate,
                    basis,
                    rounding,
                    small_business,
                    currency,
                )
                .map(|tax_line| (input.clone(), tax_line))
            })
            .collect::<Result<Vec<_>, _>>()?
    } else {
        calculate_per_document(
            lines,
            &codes,
            &rates,
            basis,
            rounding,
            small_business,
            currency,
        )?
    };

    // Sum the results.
    let mut total_net = Amount::new(0, currency);
    let mut total_output_vat = Amount::new(0, currency);
    let mut total_input_vat = Amount::new(0, currency);
    let mut total_gross = Amount::new(0, currency);

    for (_, tax_line) in &calculated_lines {
        total_net = total_net.checked_add(tax_line.net)?;
        total_output_vat = total_output_vat.checked_add(tax_line.output_vat)?;
        total_input_vat = total_input_vat.checked_add(tax_line.input_vat)?;
        total_gross = total_gross.checked_add(tax_line.gross)?;
    }

    let small_business_note = if small_business {
        Some("Gemäß § 19 UStG wird keine Umsatzsteuer berechnet.".to_string())
    } else {
        None
    };

    Ok(TaxCalculation {
        lines: calculated_lines,
        total_net,
        total_output_vat,
        total_input_vat,
        total_gross,
        small_business_note,
    })
}

/// Calculate one line's tax.
fn calculate_line(
    input: &TaxLineInput,
    code: &TaxCode,
    rate: Rate,
    basis: Basis,
    rounding: Rounding,
    small_business: bool,
    currency: Currency,
) -> Result<TaxLine, TaxError> {
    let (net, tax) = match basis {
        Basis::Net => {
            let net = input.amount;
            let tax = if small_business && code.kind == TaxKind::OutputVat {
                Amount::new(0, currency)
            } else {
                net.mul_rate(rate, rounding)?
            };
            (net, tax)
        }
        Basis::Gross => {
            let gross = input.amount;
            let tax = if small_business && code.kind == TaxKind::OutputVat {
                Amount::new(0, currency)
            } else {
                // tax = gross × r / (1 + r), so net = gross - tax and net + tax == gross
                calculate_tax_from_gross(gross, rate, rounding)?
            };
            let net = gross.checked_sub(tax)?;
            (net, tax)
        }
    };

    let (output_vat, input_vat) = match code.kind {
        TaxKind::OutputVat => (tax, Amount::new(0, currency)),
        TaxKind::InputVat | TaxKind::ImportVat => (Amount::new(0, currency), tax),
        TaxKind::ReverseCharge => (tax, tax),
        TaxKind::Exempt | TaxKind::Zero => (Amount::new(0, currency), Amount::new(0, currency)),
    };

    let gross = match basis {
        Basis::Net => net.checked_add(output_vat)?,
        Basis::Gross => input.amount,
    };

    Ok(TaxLine {
        net,
        output_vat,
        input_vat,
        gross,
    })
}

/// Calculate per-document: group lines by rate, compute tax once per group,
/// allocate to lines using largest-remainder allocation.
fn calculate_per_document(
    lines: &[TaxLineInput],
    codes: &[TaxCode],
    rates: &[Rate],
    basis: Basis,
    rounding: Rounding,
    small_business: bool,
    currency: Currency,
) -> Result<Vec<(TaxLineInput, TaxLine)>, TaxError> {
    // Group by rate.
    let mut groups: BTreeMap<i128, Vec<(usize, &TaxLineInput)>> = BTreeMap::new();
    for (i, (input, rate)) in lines.iter().zip(rates).enumerate() {
        groups.entry(rate.scaled()).or_default().push((i, input));
    }

    // Calculate tax per group and prepare allocations.
    let mut group_taxes = BTreeMap::new();
    let mut group_allocations: BTreeMap<i128, Vec<Amount>> = BTreeMap::new();

    for (rate_key, group_lines) in &groups {
        let rate = Rate::from_scaled(*rate_key);

        let group_amount: Amount = group_lines
            .iter()
            .try_fold(Amount::new(0, currency), |acc, (_, l)| {
                acc.checked_add(l.amount)
            })?;

        let group_tax = match basis {
            Basis::Net => group_amount.mul_rate(rate, rounding)?,
            Basis::Gross => {
                if small_business {
                    // Check if any code in this group is OutputVat (small business exemption)
                    let is_output_vat = group_lines
                        .iter()
                        .any(|(i, _)| codes[*i].kind == TaxKind::OutputVat);
                    if is_output_vat {
                        Amount::new(0, currency)
                    } else {
                        calculate_tax_from_gross(group_amount, rate, rounding)?
                    }
                } else {
                    calculate_tax_from_gross(group_amount, rate, rounding)?
                }
            }
        };

        // Allocate group tax to lines using their amounts as weights (largest-remainder).
        // Weights must be positive, so use the magnitude.
        let weights: Vec<i128> = group_lines
            .iter()
            .map(|(_, l)| i128::try_from(l.amount.minor().unsigned_abs()).unwrap_or(i128::MAX))
            .collect();

        let allocated_taxes = if weights.iter().all(|&w| w == 0) {
            // All zero amounts: allocate zero to all.
            vec![Amount::new(0, currency); weights.len()]
        } else {
            group_tax.allocate(&weights)?
        };

        group_taxes.insert(*rate_key, group_tax);
        group_allocations.insert(*rate_key, allocated_taxes);
    }

    // Build result: for each line, apply its allocated tax from the group.
    let mut result = Vec::new();
    let mut group_line_indices: BTreeMap<i128, usize> = BTreeMap::new();

    for (i, (input, code)) in lines.iter().zip(codes).enumerate() {
        let rate = rates[i];
        let rate_key = rate.scaled();

        let group_idx = group_line_indices.entry(rate_key).or_insert(0);
        let line_tax = group_allocations
            .get(&rate_key)
            .and_then(|allocated| allocated.get(*group_idx))
            .copied()
            .unwrap_or_else(|| Amount::new(0, currency));
        *group_idx += 1;

        let (output_vat, input_vat) = match code.kind {
            TaxKind::OutputVat => (line_tax, Amount::new(0, currency)),
            TaxKind::InputVat | TaxKind::ImportVat => (Amount::new(0, currency), line_tax),
            TaxKind::ReverseCharge => (line_tax, line_tax),
            TaxKind::Exempt | TaxKind::Zero => (Amount::new(0, currency), Amount::new(0, currency)),
        };

        let net = match basis {
            Basis::Net => input.amount,
            Basis::Gross => input.amount.checked_sub(line_tax)?,
        };

        let gross = match basis {
            Basis::Net => net.checked_add(output_vat)?,
            Basis::Gross => input.amount,
        };

        result.push((
            input.clone(),
            TaxLine {
                net,
                output_vat,
                input_vat,
                gross,
            },
        ));
    }

    Ok(result)
}

/// Calculate tax from gross: `tax = round(gross × r / (1 + r))` exactly in i128,
/// ensuring `net + tax == gross` exactly.
///
/// The rounding semantics match [`Amount::mul_rate`]: rounding acts on the
/// magnitude, then the sign is reapplied, so both positive and negative
/// amounts (credit notes) are handled symmetrically.
fn calculate_tax_from_gross(
    gross: Amount,
    rate: Rate,
    rounding: Rounding,
) -> Result<Amount, TaxError> {
    let currency = gross.currency();
    let gross_minor = gross.minor();
    let rate_scaled = rate.scaled();
    let scale = ledgers_money::rate::SCALE;

    // One + r in scaled form.
    let one_plus_r_scaled = scale
        .checked_add(rate_scaled)
        .ok_or(TaxError::Money(ledgers_money::MoneyError::Overflow))?;

    // Compute on magnitude: tax = round(|gross_minor| × r_scaled / (SCALE + r_scaled))
    let is_negative = gross_minor < 0;
    let mag = gross_minor.unsigned_abs();

    // Numerator: mag × r_scaled
    let numerator = mag
        .checked_mul(rate_scaled.unsigned_abs())
        .ok_or(TaxError::Money(ledgers_money::MoneyError::Overflow))?;

    // Division with rounding to match mul_rate semantics.
    let quotient = numerator / one_plus_r_scaled.unsigned_abs();
    let remainder = numerator % one_plus_r_scaled.unsigned_abs();

    let bump = match rounding {
        Rounding::Down => false,
        Rounding::HalfUp => {
            remainder
                .checked_mul(2)
                .ok_or(TaxError::Money(ledgers_money::MoneyError::Overflow))?
                >= one_plus_r_scaled.unsigned_abs()
        }
        Rounding::HalfEven => {
            let double = remainder
                .checked_mul(2)
                .ok_or(TaxError::Money(ledgers_money::MoneyError::Overflow))?;
            double > one_plus_r_scaled.unsigned_abs()
                || (double == one_plus_r_scaled.unsigned_abs() && quotient % 2 == 1)
        }
    };

    let tax_mag = quotient + u128::from(bump);
    let tax_minor = i128::try_from(tax_mag)
        .map_err(|_| TaxError::Money(ledgers_money::MoneyError::Overflow))?;

    // Reapply sign.
    let tax_minor_signed = if is_negative { -tax_minor } else { tax_minor };
    Ok(Amount::new(tax_minor_signed, currency))
}
