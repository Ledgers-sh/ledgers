//! The bill model: the input the commands take, the view they return, the
//! derived status, and the journal lines a bill posts as.

use ledgers_extract::checks::Check;
use ledgers_extract::schema::{ExtractedInvoice, ExtractedLine};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The flag set on a bill whose document text reads like instructions.
/// Never cleared: such a bill cannot post from the app or by an agent.
pub const FLAG_INSTRUCTION_TEXT: &str = "instruction-text";
/// The vendor was created from an extraction and no person has confirmed it.
pub const FLAG_VENDOR_NEW: &str = "vendor-new";
/// The IBAN on the invoice differs from the vendor's known IBAN: payments
/// to this vendor need a person.
pub const FLAG_IBAN_CHANGED: &str = "vendor-iban-changed";
/// A line has no expense account yet.
pub const FLAG_NO_ACCOUNT: &str = "no-expense-account";

/// The status of a bill, derived on every read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// A check failed or a flag is open.
    NeedsReview,
    /// Every check passed and nothing is open: it can be posted.
    Ready,
    /// The journal entry exists.
    Posted,
    /// A person discarded the draft; it can no longer post.
    Discarded,
}

/// One bill line as entered or extracted, with its expense account.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct BillLine {
    /// The description as printed.
    pub description: String,
    /// The net amount, minor units.
    pub net_minor: Option<i64>,
    /// The VAT rate in basis points.
    pub vat_rate_bp: Option<i64>,
    /// The VAT amount, minor units.
    pub vat_minor: Option<i64>,
    /// The gross amount, minor units.
    pub gross_minor: Option<i64>,
    /// The expense account the line debits (a chart account number).
    pub expense_account: Option<String>,
    /// The bill whose line this account was suggested from, when it was.
    pub suggested_from: Option<String>,
}

impl From<&ExtractedLine> for BillLine {
    fn from(line: &ExtractedLine) -> Self {
        BillLine {
            description: line.description.clone().unwrap_or_default(),
            net_minor: line.net_minor,
            vat_rate_bp: line.vat_rate_bp,
            vat_minor: line.vat_minor,
            gross_minor: line.gross_minor,
            expense_account: None,
            suggested_from: None,
        }
    }
}

/// The input of `bills.create`, and of `bills.update` (where the bill id comes
/// from the path and the document stays as it was). The id and the document
/// are filled by the server on create.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct BillInput {
    /// The new bill's id (server-generated on create).
    pub id: String,
    /// The document the bill is for (create only).
    pub document_id: String,
    /// The vendor's name.
    pub vendor_name: Option<String>,
    /// The vendor's VAT id.
    pub vendor_vat_id: Option<String>,
    /// The IBAN printed on the invoice.
    pub vendor_iban: Option<String>,
    /// The invoice number.
    pub invoice_number: Option<String>,
    /// The invoice date, `YYYY-MM-DD`.
    pub invoice_date: Option<String>,
    /// The due date, `YYYY-MM-DD`.
    pub due_date: Option<String>,
    /// The ISO 4217 currency.
    pub currency: Option<String>,
    /// The net total, minor units.
    pub net_minor: Option<i64>,
    /// The VAT total, minor units.
    pub vat_minor: Option<i64>,
    /// The gross total, minor units.
    pub gross_minor: Option<i64>,
    /// The lines.
    pub lines: Vec<BillLine>,
    /// Confidence per extracted field, 0 to 100 (empty for manual bills).
    pub confidence: std::collections::BTreeMap<String, u8>,
    /// Which extractor (or `manual`) produced the fields.
    pub source: Option<String>,
    /// The caller, recorded on the bill (filled by the server).
    pub created_by: String,
}

impl BillInput {
    /// The fields as an extraction, for the arithmetic checks.
    #[must_use]
    pub fn as_invoice(&self) -> ExtractedInvoice {
        ExtractedInvoice {
            vendor_name: self.vendor_name.clone(),
            vendor_vat_id: self.vendor_vat_id.clone(),
            vendor_iban: self.vendor_iban.clone(),
            invoice_number: self.invoice_number.clone(),
            invoice_date: self.invoice_date.clone(),
            due_date: self.due_date.clone(),
            currency: self.currency.clone(),
            lines: self
                .lines
                .iter()
                .map(|line| ExtractedLine {
                    description: Some(line.description.clone()),
                    net_minor: line.net_minor,
                    vat_rate_bp: line.vat_rate_bp,
                    vat_minor: line.vat_minor,
                    gross_minor: line.gross_minor,
                })
                .collect(),
            net_minor: self.net_minor,
            vat_minor: self.vat_minor,
            gross_minor: self.gross_minor,
            confidence: std::collections::BTreeMap::new(),
        }
    }
}

/// A check as the API shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CheckView {
    /// The check's id.
    pub id: String,
    /// Whether it passed.
    pub ok: bool,
    /// What it found.
    pub detail: String,
}

impl From<&Check> for CheckView {
    fn from(check: &Check) -> Self {
        CheckView {
            id: check.id.to_owned(),
            ok: check.ok,
            detail: check.detail.clone(),
        }
    }
}

/// The vendor, as the bill shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VendorView {
    /// The vendor id.
    pub id: String,
    /// The vendor's name.
    pub name: String,
    /// The VAT id, if known.
    pub vat_id: Option<String>,
    /// The known IBAN, if any.
    pub iban: Option<String>,
    /// True until a person confirms the vendor.
    pub needs_review: bool,
}

/// One line of the journal preview: account, side and amount in minor units.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PreviewLine {
    /// The account number.
    pub account: String,
    /// `debit` or `credit`.
    pub side: String,
    /// The amount, minor units of the bill currency.
    pub minor: i64,
}

/// A bill as commands and the app see it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BillView {
    /// The bill id.
    pub id: String,
    /// The derived status.
    pub status: Status,
    /// The document the bill is for.
    pub document_id: String,
    /// The vendor.
    pub vendor: VendorView,
    /// The invoice number.
    pub invoice_number: Option<String>,
    /// The invoice date.
    pub invoice_date: Option<String>,
    /// The due date.
    pub due_date: Option<String>,
    /// The currency.
    pub currency: String,
    /// Net total, minor units.
    pub net_minor: Option<i64>,
    /// VAT total, minor units.
    pub vat_minor: Option<i64>,
    /// Gross total, minor units.
    pub gross_minor: Option<i64>,
    /// The lines.
    pub lines: Vec<BillLine>,
    /// Confidence per extracted field, 0 to 100 (empty for manual bills).
    pub confidence: std::collections::BTreeMap<String, u8>,
    /// Every check on the bill.
    pub checks: Vec<CheckView>,
    /// The open flags.
    pub flags: Vec<String>,
    /// Which extractor produced the fields.
    pub source: String,
    /// The posted journal entry, once posted.
    pub posted_entry_id: Option<String>,
    /// The entry the bill posts as, when every amount and account is known.
    pub journal_preview: Vec<PreviewLine>,
    /// Who created the bill.
    pub created_by: String,
    /// When it was created.
    pub created_at: String,
    /// When it last changed.
    pub updated_at: String,
}

/// The status a bill has now, from its stored state.
#[must_use]
pub fn derive_status(
    posted: bool,
    discarded: bool,
    checks: &[CheckView],
    flags: &[String],
) -> Status {
    if posted {
        Status::Posted
    } else if discarded {
        Status::Discarded
    } else if checks.iter().all(|c| c.ok) && flags.is_empty() {
        Status::Ready
    } else {
        Status::NeedsReview
    }
}

/// The debit and credit lines a bill posts as, in minor units: one debit per
/// line on its expense account, one debit of the input VAT total on the input
/// VAT account, one credit of the gross on accounts payable. Zero amounts
/// are left out. The journal entry and the preview are both built from this.
#[must_use]
pub fn entry_lines(
    lines: &[BillLine],
    vat_minor: i64,
    gross_minor: i64,
    vat_account: &str,
    payable_account: &str,
) -> Vec<PreviewLine> {
    let mut out = Vec::new();
    for line in lines {
        if let (Some(net), Some(account)) = (line.net_minor, line.expense_account.as_deref())
            && net != 0
        {
            out.push(PreviewLine {
                account: account.to_owned(),
                side: "debit".to_owned(),
                minor: net,
            });
        }
    }
    if vat_minor != 0 {
        out.push(PreviewLine {
            account: vat_account.to_owned(),
            side: "debit".to_owned(),
            minor: vat_minor,
        });
    }
    out.push(PreviewLine {
        account: payable_account.to_owned(),
        side: "credit".to_owned(),
        minor: gross_minor,
    });
    out
}

/// The flags a bill has open: the stored instruction flags, the vendor state,
/// the IBAN state, and a missing expense account.
#[must_use]
pub fn open_flags(
    instruction_flags: &[String],
    vendor_needs_review: bool,
    iban_changed: bool,
    lines: &[BillLine],
) -> Vec<String> {
    let mut flags: Vec<String> = instruction_flags.to_vec();
    if vendor_needs_review {
        flags.push(FLAG_VENDOR_NEW.to_owned());
    }
    if iban_changed {
        flags.push(FLAG_IBAN_CHANGED.to_owned());
    }
    if !lines.is_empty() && lines.iter().any(|line| line.expense_account.is_none()) {
        flags.push(FLAG_NO_ACCOUNT.to_owned());
    }
    flags
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_check() -> CheckView {
        CheckView {
            id: "x".into(),
            ok: true,
            detail: String::new(),
        }
    }

    #[test]
    fn status_is_ready_only_with_no_failed_check_and_no_flag() {
        let bad = CheckView {
            ok: false,
            ..ok_check()
        };
        assert_eq!(
            derive_status(false, false, &[ok_check()], &[]),
            Status::Ready
        );
        assert_eq!(
            derive_status(false, false, &[bad], &[]),
            Status::NeedsReview
        );
        assert_eq!(
            derive_status(false, false, &[ok_check()], &[FLAG_VENDOR_NEW.into()]),
            Status::NeedsReview
        );
        assert_eq!(derive_status(true, false, &[], &[]), Status::Posted);
        assert_eq!(
            derive_status(false, true, &[ok_check()], &[]),
            Status::Discarded
        );
    }

    #[test]
    fn the_entry_balances_when_the_bill_adds_up() {
        let lines = vec![BillLine {
            net_minor: Some(100_000),
            expense_account: Some("5060".into()),
            ..BillLine::default()
        }];
        let entry = entry_lines(&lines, 19_000, 119_000, "1050", "2010");
        let debit: i64 = entry
            .iter()
            .filter(|l| l.side == "debit")
            .map(|l| l.minor)
            .sum();
        let credit: i64 = entry
            .iter()
            .filter(|l| l.side == "credit")
            .map(|l| l.minor)
            .sum();
        assert_eq!(debit, credit);
        assert!(
            entry
                .iter()
                .any(|l| l.account == "1050" && l.minor == 19_000)
        );
    }

    #[test]
    fn a_missing_expense_account_is_an_open_flag() {
        let lines = vec![BillLine {
            net_minor: Some(1),
            ..BillLine::default()
        }];
        assert_eq!(
            open_flags(&[], false, false, &lines),
            vec![FLAG_NO_ACCOUNT.to_owned()]
        );
    }
}
