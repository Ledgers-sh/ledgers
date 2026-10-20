//! Tax codes (the module for codes; settings are in [`crate::settings`]).

use crate::error::TaxError;
use ledgers_money::Rate;

pub use crate::settings::{TaxSettings, VatBasis};

/// The kind of tax the code applies.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum TaxKind {
    /// Output VAT (sales tax collected from customers).
    OutputVat,
    /// Input VAT (tax paid on purchases, deductible).
    InputVat,
    /// Reverse-charge: both output and input VAT posted (intra-EU acquisitions).
    ReverseCharge,
    /// Exempt: no VAT charged or deductible.
    Exempt,
    /// Zero rate (e.g., intra-EU supplies).
    Zero,
    /// Import VAT (input only, self-assessed).
    ImportVat,
}

impl TaxKind {
    /// Parse from a string.
    ///
    /// # Errors
    ///
    /// Returns [`TaxError::Invalid`] if the code is not recognized.
    pub fn from_code(code: &str) -> Result<Self, TaxError> {
        match code.to_lowercase().as_str() {
            "output_vat" => Ok(TaxKind::OutputVat),
            "input_vat" => Ok(TaxKind::InputVat),
            "reverse_charge" => Ok(TaxKind::ReverseCharge),
            "exempt" => Ok(TaxKind::Exempt),
            "zero" => Ok(TaxKind::Zero),
            "import_vat" => Ok(TaxKind::ImportVat),
            _ => Err(TaxError::Invalid {
                reason: format!(
                    "unknown tax kind {code:?}, expected output_vat|input_vat|reverse_charge|exempt|zero|import_vat"
                ),
            }),
        }
    }

    /// The kind as a string.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            TaxKind::OutputVat => "output_vat",
            TaxKind::InputVat => "input_vat",
            TaxKind::ReverseCharge => "reverse_charge",
            TaxKind::Exempt => "exempt",
            TaxKind::Zero => "zero",
            TaxKind::ImportVat => "import_vat",
        }
    }
}

impl std::fmt::Display for TaxKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

/// A tax code configuration.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TaxCode {
    /// The code's unique identifier (e.g., `"19-U"` for 19% output VAT).
    pub code: String,
    /// The kind of tax.
    pub kind: TaxKind,
    /// The rate category (standard, reduced, etc.) — used to look up
    /// the actual rate in the rate table.
    pub category: crate::rate::RateCategory,
    /// The account to post output VAT to (needed for `OutputVat` and
    /// `ReverseCharge`).
    pub output_account: Option<String>,
    /// The account to post input VAT to (needed for `InputVat`,
    /// `ReverseCharge`, and `ImportVat`).
    pub input_account: Option<String>,
    /// The non-deductible share of input VAT: 0 = fully deductible,
    /// 1 = not deductible, 0.5 = half deductible. The non-deductible
    /// portion is debited to the line's own account.
    pub non_deductible: Rate,
}

impl TaxCode {
    /// Validate and construct a tax code.
    ///
    /// # Errors
    /// [`TaxError::InvalidNonDeductible`] if the share is not in [0, 1].
    /// [`TaxError::MissingAccount`] if a required account is not provided
    /// for the kind.
    pub fn new(
        code: String,
        kind: TaxKind,
        category: crate::rate::RateCategory,
        output_account: Option<String>,
        input_account: Option<String>,
        non_deductible: Rate,
    ) -> Result<Self, TaxError> {
        // Validate non-deductible is in [0, 1].
        if non_deductible.scaled() < 0 || non_deductible.scaled() > ledgers_money::rate::SCALE {
            return Err(TaxError::InvalidNonDeductible {
                value: non_deductible.to_string(),
            });
        }

        // Validate required accounts are present.
        match kind {
            TaxKind::OutputVat | TaxKind::ReverseCharge if output_account.is_none() => {
                return Err(TaxError::MissingAccount {
                    code,
                    kind: kind.to_string(),
                    missing: "output".to_string(),
                });
            }
            TaxKind::InputVat | TaxKind::ImportVat if input_account.is_none() => {
                return Err(TaxError::MissingAccount {
                    code,
                    kind: kind.to_string(),
                    missing: "input".to_string(),
                });
            }
            _ => {}
        }
        // ReverseCharge requires both, check input here too
        if kind == TaxKind::ReverseCharge && input_account.is_none() {
            return Err(TaxError::MissingAccount {
                code,
                kind: kind.to_string(),
                missing: "input".to_string(),
            });
        }

        Ok(TaxCode {
            code,
            kind,
            category,
            output_account,
            input_account,
            non_deductible,
        })
    }
}
