//! Document screening before money moves.

use serde::{Deserialize, Serialize};
use time::Date;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BankDetails {
    iban: String,
}

impl BankDetails {
    /// Creates a new `BankDetails` from an IBAN string.
    ///
    /// # Errors
    ///
    /// Returns an error if the IBAN is invalid (wrong length, format, or checksum).
    pub fn new(iban: &str) -> Result<Self, String> {
        let n = iban.to_uppercase().replace(' ', "");
        if n.len() < 15 || n.len() > 34 {
            return Err("IBAN length invalid".to_string());
        }
        if n.len() < 4
            || !n[0..2].chars().all(|c| c.is_ascii_alphabetic())
            || !n[2..4].chars().all(|c| c.is_ascii_digit())
        {
            return Err("IBAN format invalid".to_string());
        }
        if !validate_iban_checksum(&n) {
            return Err("IBAN checksum failed".to_string());
        }
        Ok(BankDetails { iban: n })
    }

    #[must_use]
    pub fn iban(&self) -> &str {
        &self.iban
    }
}

fn validate_iban_checksum(iban: &str) -> bool {
    let mut num = String::new();
    for c in format!("{}{}", &iban[4..], &iban[..4]).chars() {
        if c.is_ascii_digit() {
            num.push(c);
        } else if c.is_ascii_alphabetic() {
            num.push_str(&(c as u32 - 'A' as u32 + 10).to_string());
        }
    }
    let mut r: u32 = 0;
    for c in num.chars() {
        r = (r * 10 + c.to_digit(10).unwrap_or(0)) % 97;
    }
    r == 1
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BillCandidate {
    pub vendor: String,
    pub number: String,
    pub currency: String,
    pub total: i128,
    pub issue_date: Date,
    pub document_hash: String,
    pub bank: Option<BankDetails>,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct KnownBill {
    pub number: String,
    pub total: i128,
    pub currency: String,
    pub issue_date: Date,
    pub document_hash: String,
}

#[derive(Clone, Debug)]
pub struct ScreenContext {
    pub vendor_bank_details: Option<BankDetails>,
    pub prior_bills: Vec<KnownBill>,
    pub agent_bills_24h: u32,
    pub agent_baseline_24h: u32,
    pub per_entry_limit: Option<i128>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskFlag {
    BankDetailsChanged,
    NewVendor,
    DuplicateBill,
    DuplicateDocument,
    VolumeSpike,
    RoundAmountUnderLimit,
    InstructionText,
}

impl RiskFlag {
    #[must_use]
    pub fn reason(&self) -> &'static str {
        match self {
            RiskFlag::BankDetailsChanged => "bank details changed",
            RiskFlag::NewVendor => "new vendor",
            RiskFlag::DuplicateBill => "duplicate bill",
            RiskFlag::DuplicateDocument => "duplicate document",
            RiskFlag::VolumeSpike => "volume spike",
            RiskFlag::RoundAmountUnderLimit => "round amount near limit",
            RiskFlag::InstructionText => "instruction text",
        }
    }
}

#[must_use]
pub fn screen(candidate: &BillCandidate, ctx: &ScreenContext) -> Vec<RiskFlag> {
    const DUP_WINDOW: i64 = 45;
    let mut flags = std::collections::BTreeSet::new();

    if let (Some(known_bank), Some(cand_bank)) = (&ctx.vendor_bank_details, &candidate.bank)
        && cand_bank.iban() != known_bank.iban()
    {
        flags.insert(RiskFlag::BankDetailsChanged);
    }

    if ctx.prior_bills.is_empty() {
        flags.insert(RiskFlag::NewVendor);
    }

    for known in &ctx.prior_bills {
        if known.number == candidate.number
            && known.currency == candidate.currency
            && known.total == candidate.total
        {
            let days = (candidate.issue_date - known.issue_date).whole_days();
            if days.abs() <= DUP_WINDOW {
                flags.insert(RiskFlag::DuplicateBill);
                break;
            }
        }
        if known.document_hash == candidate.document_hash {
            flags.insert(RiskFlag::DuplicateDocument);
            break;
        }
    }

    if ctx.agent_baseline_24h > 0 {
        // spike when count * 2 > baseline * 3 (i.e. > 1.5×)
        if ctx.agent_bills_24h * 2 > ctx.agent_baseline_24h * 3 {
            flags.insert(RiskFlag::VolumeSpike);
        }
    }

    if let Some(limit) = ctx.per_entry_limit
        && candidate.total > 0
        && candidate.total < limit
        && candidate.total % 100 == 0
        && candidate.total * 10 >= limit * 9
    {
        // round-under-limit when total * 10 >= limit * 9 && total < limit
        flags.insert(RiskFlag::RoundAmountUnderLimit);
    }

    let text_lower = candidate.text.to_lowercase();
    let patterns = [
        "ignore previous",
        "system prompt",
        "you are now",
        "approve this",
        "do not flag",
        "do not check",
        "skip check",
        "no approval",
    ];
    if patterns.iter().any(|p| text_lower.contains(p)) {
        flags.insert(RiskFlag::InstructionText);
    }

    flags.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iban_valid() {
        assert!(BankDetails::new("DE89 3704 0044 0532 0130 00").is_ok());
    }

    #[test]
    fn iban_invalid_checksum() {
        assert!(BankDetails::new("DE89 3704 0044 0532 0130 01").is_err());
    }

    #[test]
    fn iban_invalid_length() {
        assert!(BankDetails::new("DE89").is_err());
    }
}
