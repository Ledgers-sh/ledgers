//! The journal entries an intercompany period becomes, one entry per side and
//! per step. The lines are the ledger's own input shape, so the company's
//! executor posts them with the same checks as any entry.
//!
//! Accrual (per period):
//! - licensee: Dr royalty expense (gross), Cr intercompany payable (net),
//!   Cr withholding payable (withholding);
//! - licensor: Dr intercompany receivable (gross), Cr royalty income (gross).
//!
//! Settlement (after the accrual):
//! - licensee: Dr intercompany payable (net), Cr bank (net);
//! - licensor: Dr bank (net), Dr withholding receivable (withholding),
//!   Cr intercompany receivable (gross).
//!
//! Every entry balances in the agreement currency, which is the base currency
//! of both companies. A zero withholding leaves its line out.

use crate::model::{Accounts, PeriodAmounts, Side};
use ledgers_commands::CommandError;
use ledgers_ledger::surface::{
    PostEntryInput, SerializableAmount, SerializableEntryDraft, SerializableLine,
};
use std::collections::BTreeMap;

/// Which step an entry belongs to.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// The period's accrual.
    Accrue,
    /// The payment that settles the accrual.
    Settle,
}

impl Step {
    /// The text the external id carries.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Step::Accrue => "accrue",
            Step::Settle => "settle",
        }
    }
}

/// The external id of one entry: `ic-<agreement>-<period>-<side>-<step>-<suffix>`.
/// The suffix is unique per attempt (the accrual's attempt number, the
/// settlement's call id), so an entry that was reversed is never returned
/// for a later attempt: the ledger answers a repeated external id with the
/// entry it already holds.
#[must_use]
pub fn external_id(
    agreement_id: &str,
    period: &str,
    side: Side,
    step: Step,
    suffix: &str,
) -> String {
    format!(
        "ic-{agreement_id}-{period}-{}-{}-{suffix}",
        side.as_str(),
        step.as_str()
    )
}

/// What an entry needs to be built.
pub struct EntryContext<'a> {
    /// The general journal the entry posts into.
    pub journal_id: &'a str,
    /// The fiscal year of the entry number.
    pub fiscal_year: i64,
    /// The last day of the period, the entry's date.
    pub period_end: &'a str,
    /// The period, `YYYY-MM`.
    pub period: &'a str,
    /// The agreement's id.
    pub agreement_id: &'a str,
    /// The agreement currency.
    pub currency: &'a str,
    /// The suffix that makes the external id unique to this attempt.
    pub suffix: &'a str,
}

fn line(account: &str, side: &str, minor: i64, currency: &str) -> SerializableLine {
    SerializableLine {
        account_number: account.to_owned(),
        side: side.to_owned(),
        base: SerializableAmount {
            minor: minor.to_string(),
            currency: currency.to_owned(),
        },
        fx: None,
        tax_code: None,
        dimensions: BTreeMap::new(),
        memo: None,
    }
}

/// A royalty line: a P&L line, so it carries the side's business unit when
/// the company has units.
fn royalty_line(
    account: &str,
    side: &str,
    minor: i64,
    currency: &str,
    unit: Option<&str>,
) -> SerializableLine {
    let mut out = line(account, side, minor, currency);
    if let Some(code) = unit {
        out.dimensions.insert("unit".to_owned(), code.to_owned());
    }
    out
}

/// The lines of one side's entry for one step.
///
/// # Errors
/// [`CommandError::InvalidInput`] when the withholding is not zero but the
/// side names no withholding account, or the lines do not balance.
pub fn lines_for(
    side: Side,
    step: Step,
    accounts: &Accounts,
    amounts: &PeriodAmounts,
    currency: &str,
) -> Result<Vec<SerializableLine>, CommandError> {
    let PeriodAmounts {
        gross_minor: gross,
        wht_minor: wht,
        net_minor: net,
        ..
    } = *amounts;
    let wht_account = match (wht, &accounts.wht) {
        (0, _) => None,
        (_, Some(account)) => Some(account.as_str()),
        (_, None) => {
            return Err(CommandError::InvalidInput(
                "the agreement withholds tax but names no withholding account".into(),
            ));
        }
    };
    let unit = accounts.unit.as_deref();
    let mut lines = match (side, step) {
        (Side::Licensee, Step::Accrue) => {
            let mut lines = vec![
                royalty_line(&accounts.royalty, "debit", gross, currency, unit),
                line(&accounts.ic, "credit", net, currency),
            ];
            if let Some(account) = wht_account {
                lines.push(line(account, "credit", wht, currency));
            }
            lines
        }
        (Side::Licensor, Step::Accrue) => vec![
            line(&accounts.ic, "debit", gross, currency),
            royalty_line(&accounts.royalty, "credit", gross, currency, unit),
        ],
        (Side::Licensee, Step::Settle) => vec![
            line(&accounts.ic, "debit", net, currency),
            line(&accounts.bank, "credit", net, currency),
        ],
        (Side::Licensor, Step::Settle) => {
            let mut lines = vec![line(&accounts.bank, "debit", net, currency)];
            if let Some(account) = wht_account {
                lines.push(line(account, "debit", wht, currency));
            }
            lines.push(line(&accounts.ic, "credit", gross, currency));
            lines
        }
    };
    lines.retain(|l| l.base.minor != "0");
    check_balanced(&lines)?;
    Ok(lines)
}

/// Both totals of a line set, debit and credit, must match.
fn check_balanced(lines: &[SerializableLine]) -> Result<(), CommandError> {
    let mut debit: i128 = 0;
    let mut credit: i128 = 0;
    for l in lines {
        let amount: i128 = l
            .base
            .minor
            .parse()
            .map_err(|_| CommandError::InvalidInput("an entry line is not an amount".into()))?;
        if l.side == "debit" {
            debit += amount;
        } else {
            credit += amount;
        }
    }
    if debit == credit {
        Ok(())
    } else {
        Err(CommandError::InvalidInput(format!(
            "the intercompany entry does not balance: debit {debit}, credit {credit}"
        )))
    }
}

/// The entry input one side posts for one step.
///
/// # Errors
/// As [`lines_for`].
pub fn entry_for(
    side: Side,
    step: Step,
    accounts: &Accounts,
    amounts: &PeriodAmounts,
    context: &EntryContext<'_>,
) -> Result<PostEntryInput, CommandError> {
    let lines = lines_for(side, step, accounts, amounts, context.currency)?;
    let description = match step {
        Step::Accrue => format!(
            "Intercompany accrual {} {}",
            context.agreement_id, context.period
        ),
        Step::Settle => format!(
            "Intercompany settlement {} {}",
            context.agreement_id, context.period
        ),
    };
    Ok(PostEntryInput {
        journal_id: context.journal_id.to_owned(),
        fiscal_year: context.fiscal_year,
        entry: SerializableEntryDraft {
            journal_id: context.journal_id.to_owned(),
            document_date: context.period_end.to_owned(),
            posting_date: context.period_end.to_owned(),
            tax_point_date: None,
            base_currency: context.currency.to_owned(),
            description,
            source_documents: Vec::new(),
            external_id: Some(external_id(
                context.agreement_id,
                context.period,
                side,
                step,
                context.suffix,
            )),
            source_app: Some("ledgers-group".to_owned()),
            reverses: None,
            lines,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Accounts;

    fn accounts(with_wht: bool) -> Accounts {
        Accounts {
            royalty: "5120".into(),
            ic: "2060".into(),
            wht: with_wht.then(|| "2070".into()),
            bank: "1020".into(),
            unit: None,
        }
    }

    fn amounts(gross: i64, wht: i64) -> PeriodAmounts {
        PeriodAmounts {
            basis_minor: gross,
            gross_minor: gross,
            wht_minor: wht,
            net_minor: gross - wht,
        }
    }

    fn summary(lines: &[SerializableLine]) -> Vec<(String, String, String)> {
        lines
            .iter()
            .map(|l| {
                (
                    l.account_number.clone(),
                    l.side.clone(),
                    l.base.minor.clone(),
                )
            })
            .collect()
    }

    #[test]
    fn licensee_accrual_splits_gross_into_payable_and_withholding() {
        let lines = lines_for(
            Side::Licensee,
            Step::Accrue,
            &accounts(true),
            &amounts(50_000, 5_000),
            "SGD",
        )
        .expect("lines");
        assert_eq!(
            summary(&lines),
            vec![
                ("5120".into(), "debit".into(), "50000".into()),
                ("2060".into(), "credit".into(), "45000".into()),
                ("2070".into(), "credit".into(), "5000".into()),
            ]
        );
    }

    #[test]
    fn a_zero_withholding_leaves_its_line_out() {
        let lines = lines_for(
            Side::Licensee,
            Step::Accrue,
            &accounts(true),
            &amounts(50_000, 0),
            "SGD",
        )
        .expect("lines");
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn licensor_settlement_clears_the_receivable_gross() {
        let lines = lines_for(
            Side::Licensor,
            Step::Settle,
            &accounts(true),
            &amounts(50_000, 5_000),
            "SGD",
        )
        .expect("lines");
        assert_eq!(
            summary(&lines),
            vec![
                ("1020".into(), "debit".into(), "45000".into()),
                ("2070".into(), "debit".into(), "5000".into()),
                ("2060".into(), "credit".into(), "50000".into()),
            ]
        );
    }

    #[test]
    fn withholding_without_an_account_is_refused() {
        let err = lines_for(
            Side::Licensee,
            Step::Accrue,
            &accounts(false),
            &amounts(50_000, 5_000),
            "SGD",
        )
        .expect_err("refused");
        assert!(matches!(err, CommandError::InvalidInput(_)));
    }

    #[test]
    fn external_ids_carry_the_attempt_and_the_agreement_prefix() {
        assert_eq!(
            external_id("01AGR", "2026-01", Side::Licensor, Step::Accrue, "2"),
            "ic-01AGR-2026-01-licensor-accrue-2"
        );
    }
}
