//! The group and agreement vocabulary: the enums the rows store, the row views
//! the commands return, and the exact integer arithmetic for amounts and
//! withholding.

use ledgers_tax::CountryPack;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The longest group or agreement name a person may give.
pub const NAME_MAX: usize = 120;
/// The most periods one command call may run or post.
pub const MAX_PERIODS: usize = 120;

macro_rules! string_enum {
    ($name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        impl $name {
            /// The stored and wire text of the variant.
            #[must_use]
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }

            /// The variant named by `text`, if any.
            #[must_use]
            pub fn parse(text: &str) -> Option<Self> {
                match text {
                    $($text => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

/// What a group member is in the group.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GroupKind {
    /// A holding company.
    Holding,
    /// The company that owns the intellectual property.
    IpHolding,
    /// A company that trades and pays for the IP.
    Operating,
}

string_enum!(GroupKind {
    Holding => "holding",
    IpHolding => "ip_holding",
    Operating => "operating",
});

/// The kind of intercompany agreement.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A licence to use intellectual property.
    Licence,
    /// A royalty for intellectual property.
    Royalty,
    /// A management fee.
    ManagementFee,
    /// A loan.
    Loan,
}

string_enum!(Kind {
    Licence => "licence",
    Royalty => "royalty",
    ManagementFee => "management_fee",
    Loan => "loan",
});

impl Kind {
    /// Whether withholding tax on royalties applies to this kind.
    #[must_use]
    pub fn is_royalty(self) -> bool {
        matches!(self, Kind::Licence | Kind::Royalty)
    }
}

/// How a period's amount is worked out.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    /// A rate in basis points of the licensee's revenue for the period.
    PctOfRevenueBp,
    /// A fixed amount each month.
    FixedMonthly,
    /// A mark-up in basis points on the licensor's expenses for the period.
    CostPlusBp,
}

string_enum!(Basis {
    PctOfRevenueBp => "pct_of_revenue_bp",
    FixedMonthly => "fixed_monthly",
    CostPlusBp => "cost_plus_bp",
});

/// Which party a row belongs to.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    /// The party that owns the rights and receives the income.
    Licensor,
    /// The party that pays and books the expense.
    Licensee,
}

string_enum!(Side {
    Licensor => "licensor",
    Licensee => "licensee",
});

impl Side {
    /// Both sides, licensor first: the order posting runs in.
    pub const BOTH: [Side; 2] = [Side::Licensor, Side::Licensee];
}

/// Where a period's document stands.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DocStatus {
    /// Drafted, not posted.
    Draft,
    /// The accrual is posted.
    Posted,
    /// The accrual and the settlement are posted.
    Settled,
    /// A post attempt was reversed; the next post starts again.
    Failed,
}

string_enum!(DocStatus {
    Draft => "draft",
    Posted => "posted",
    Settled => "settled",
    Failed => "failed",
});

/// Why a withholding rate is what it is.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WhtBasis {
    /// The payer's country pack gives the rate.
    Pack,
    /// A treaty rate the person entered replaces the pack rate.
    TreatyOverride,
    /// The payee is resident in the payer's country: no withholding.
    Resident,
    /// The pack has no verified rate for this payment: none is applied.
    NoRateOnFile,
    /// The kind of agreement carries no withholding.
    NotApplicable,
}

string_enum!(WhtBasis {
    Pack => "pack",
    TreatyOverride => "treaty_override",
    Resident => "resident",
    NoRateOnFile => "no_rate_on_file",
    NotApplicable => "not_applicable",
});

/// The accounts one side posts to, by number.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Accounts {
    /// The royalty account: an expense for the licensee, income for the licensor.
    pub royalty: String,
    /// The intercompany account: a payable for the licensee, a receivable for the licensor.
    pub ic: String,
    /// The withholding account: a payable for the licensee, a receivable for the licensor.
    pub wht: Option<String>,
    /// The bank account the settlement moves money through.
    pub bank: String,
    /// The business unit the royalty line carries, required when the company
    /// has business units (its P&L lines must name one).
    pub unit: Option<String>,
}

/// The accounts one side names in a create or update call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AccountsInput {
    /// The royalty account number.
    pub royalty_account: String,
    /// The intercompany account number.
    pub ic_account: String,
    /// The withholding account number, when the agreement has withholding.
    pub wht_account: Option<String>,
    /// The bank account number.
    pub bank_account: String,
    /// The business unit the royalty line carries. Required when the company
    /// has business units.
    #[serde(default)]
    pub unit: Option<String>,
}

/// One row of an agreement, as one side sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgreementView {
    /// The agreement id.
    pub id: String,
    /// The group the agreement belongs to.
    pub group_id: String,
    /// This company's side.
    pub side: Side,
    /// The other company, when the person may see it; `None` otherwise.
    pub counterparty_id: Option<String>,
    /// True when the other company exists but the person may not see it.
    pub counterparty_hidden: bool,
    /// The kind.
    pub kind: Kind,
    /// The basis.
    pub basis: Basis,
    /// The rate in basis points, for the two percentage bases.
    pub rate_bp: Option<i64>,
    /// The fixed monthly amount in minor units, for the fixed basis.
    pub fixed_minor: Option<i64>,
    /// The currency, the base currency of both companies.
    pub currency: String,
    /// The withholding rate applied to each period, in basis points.
    pub wht_bp: i64,
    /// Why that rate applies.
    pub wht_basis: WhtBasis,
    /// The treaty rate the person entered, if any.
    pub treaty_override_bp: Option<i64>,
    /// This side's accounts.
    pub accounts: Accounts,
    /// Free text: the transfer pricing note and the agreement reference.
    pub notes: Option<String>,
    /// When the agreement stopped producing periods.
    pub terminated_at: Option<String>,
    /// Who created it.
    pub created_by: String,
    /// When created.
    pub created_at: String,
    /// When last changed.
    pub updated_at: String,
}

/// One period of an agreement for one side.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PeriodDocView {
    /// The intercompany id, shared by both sides.
    pub id: String,
    /// The side.
    pub side: Side,
    /// The period, `YYYY-MM`.
    pub period: String,
    /// The status.
    pub status: DocStatus,
    /// The currency.
    pub currency: String,
    /// The amount the basis produced, in minor units.
    pub basis_minor: i64,
    /// The royalty amount before withholding.
    pub gross_minor: i64,
    /// The withholding tax.
    pub wht_minor: i64,
    /// Gross less withholding: what the licensee owes and pays.
    pub net_minor: i64,
    /// The licensor's or licensee's accrual entry, once posted.
    pub accrual_entry_id: Option<String>,
    /// The settlement entry, once settled.
    pub settle_entry_id: Option<String>,
    /// Why the last post attempt failed, when it did.
    pub last_error: Option<String>,
}

/// The base-currency amount of the company's own P&L line, as the period's
/// basis reads it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BasisInputs {
    /// The licensee's revenue for the period (basis `pct_of_revenue_bp`).
    pub revenue_minor: i64,
    /// The licensor's expenses for the period (basis `cost_plus_bp`).
    pub expenses_minor: i64,
}

/// The amounts one period produces, in minor units of the agreement currency.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PeriodAmounts {
    /// What the basis produced.
    pub basis_minor: i64,
    /// The royalty before withholding.
    pub gross_minor: i64,
    /// The withholding tax on the gross.
    pub wht_minor: i64,
    /// The gross less withholding.
    pub net_minor: i64,
}

/// Rounds `numerator / denominator` half away from zero, for a non-negative
/// numerator and a positive denominator. This is the one rounding rule the
/// group uses, and it is named here so every amount follows it.
#[must_use]
pub fn round_half_up(numerator: i128, denominator: i128) -> i128 {
    (numerator * 2 + denominator) / (denominator * 2)
}

/// `amount` times `bp` basis points, rounded half up to the minor unit.
///
/// # Errors
/// [`None`] when the product does not fit the storage width.
#[must_use]
pub fn bp_of(amount_minor: i64, bp: i64) -> Option<i64> {
    let amount = i128::from(amount_minor.max(0));
    let value = round_half_up(amount.checked_mul(i128::from(bp))?, 10_000);
    i64::try_from(value).ok()
}

/// The amounts of one period from the agreement's basis and the period's
/// inputs. A negative revenue or expense base produces nothing.
///
/// # Errors
/// [`None`] when an amount does not fit the storage width.
#[must_use]
pub fn period_amounts(
    basis: Basis,
    rate_bp: Option<i64>,
    fixed_minor: Option<i64>,
    wht_bp: i64,
    inputs: BasisInputs,
) -> Option<PeriodAmounts> {
    let basis_minor = match basis {
        Basis::PctOfRevenueBp => bp_of(inputs.revenue_minor, rate_bp.unwrap_or(0))?,
        Basis::CostPlusBp => bp_of(inputs.expenses_minor, rate_bp.unwrap_or(0))?,
        Basis::FixedMonthly => fixed_minor.unwrap_or(0),
    };
    let gross_minor = basis_minor;
    let wht_minor = bp_of(gross_minor, wht_bp)?;
    Some(PeriodAmounts {
        basis_minor,
        gross_minor,
        wht_minor,
        net_minor: gross_minor - wht_minor,
    })
}

/// Parses a `YYYY-MM` period text.
#[must_use]
pub fn parse_period(text: &str) -> Option<(i32, u8)> {
    let (year, month) = text.split_once('-')?;
    if year.len() != 4 || month.len() != 2 {
        return None;
    }
    let year: i32 = year.parse().ok()?;
    let month: u8 = month.parse().ok()?;
    (1..=12).contains(&month).then_some((year, month))
}

/// The last calendar day of `YYYY-MM`, as `YYYY-MM-DD`.
#[must_use]
pub fn period_end(text: &str) -> Option<String> {
    let (year, month) = parse_period(text)?;
    let month_value = time::Month::try_from(month).ok()?;
    let days = month_value.length(year);
    Some(format!("{year:04}-{month:02}-{days:02}"))
}

/// The withholding rate and its reason for one agreement.
///
/// Withholding applies when a resident company pays a non-resident for a
/// licence or royalty. A payee in the payer's own country is resident; the
/// treaty override replaces the pack rate only for a non-resident payee.
#[must_use]
pub fn withholding_for(
    kind: Kind,
    paying: Option<&CountryPack>,
    receiving: Option<&CountryPack>,
    treaty_override_bp: Option<i64>,
) -> (i64, WhtBasis) {
    if !kind.is_royalty() {
        return (0, WhtBasis::NotApplicable);
    }
    if let (Some(paying), Some(receiving)) = (paying, receiving)
        && paying.id == receiving.id
    {
        return (0, WhtBasis::Resident);
    }
    if let Some(bp) = treaty_override_bp {
        return (bp, WhtBasis::TreatyOverride);
    }
    let rate = paying.and_then(|pack| {
        pack.withholding
            .iter()
            .find(|entry| entry.kind == "royalty")
            .and_then(|entry| entry.rate_bp)
    });
    match rate {
        Some(bp) => (bp, WhtBasis::Pack),
        None => (0, WhtBasis::NoRateOnFile),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_is_half_up_and_exact() {
        assert_eq!(round_half_up(5, 10), 1);
        assert_eq!(round_half_up(4, 10), 0);
        assert_eq!(round_half_up(15, 10), 2);
    }

    #[test]
    fn five_percent_of_revenue_in_minor_units() {
        // 5% of 1,234,567 cents is 61,728.35, which rounds to 61,728.
        assert_eq!(bp_of(1_234_567, 500), Some(61_728));
        // Half a cent rounds up.
        assert_eq!(bp_of(10, 500), Some(1));
    }

    #[test]
    fn withholding_splits_gross_into_net_and_tax_exactly() {
        let amounts = period_amounts(
            Basis::PctOfRevenueBp,
            Some(500),
            None,
            1000,
            BasisInputs {
                revenue_minor: 1_000_000,
                expenses_minor: 0,
            },
        )
        .expect("amounts");
        assert_eq!(amounts.basis_minor, 50_000);
        assert_eq!(amounts.gross_minor, 50_000);
        assert_eq!(amounts.wht_minor, 5_000);
        assert_eq!(amounts.net_minor, 45_000);
        assert_eq!(amounts.gross_minor, amounts.net_minor + amounts.wht_minor);
    }

    #[test]
    fn a_negative_base_produces_nothing() {
        let amounts = period_amounts(
            Basis::PctOfRevenueBp,
            Some(500),
            None,
            0,
            BasisInputs {
                revenue_minor: -900,
                expenses_minor: 0,
            },
        )
        .expect("amounts");
        assert_eq!(amounts.gross_minor, 0);
    }

    #[test]
    fn cost_plus_reads_the_licensors_expenses() {
        let amounts = period_amounts(
            Basis::CostPlusBp,
            Some(1000),
            None,
            0,
            BasisInputs {
                revenue_minor: 0,
                expenses_minor: 20_000,
            },
        )
        .expect("amounts");
        assert_eq!(amounts.gross_minor, 2_000);
    }

    #[test]
    fn period_end_is_the_last_day_of_the_month() {
        assert_eq!(period_end("2026-02").as_deref(), Some("2026-02-28"));
        assert_eq!(period_end("2028-02").as_deref(), Some("2028-02-29"));
        assert_eq!(period_end("2026-12").as_deref(), Some("2026-12-31"));
        assert_eq!(period_end("2026-13"), None);
        assert_eq!(period_end("26-01"), None);
    }

    #[test]
    fn enum_text_round_trips() {
        for kind in [
            Kind::Licence,
            Kind::Royalty,
            Kind::ManagementFee,
            Kind::Loan,
        ] {
            assert_eq!(Kind::parse(kind.as_str()), Some(kind));
        }
        for basis in [
            Basis::PctOfRevenueBp,
            Basis::FixedMonthly,
            Basis::CostPlusBp,
        ] {
            assert_eq!(Basis::parse(basis.as_str()), Some(basis));
        }
        assert_eq!(GroupKind::parse("ip_holding"), Some(GroupKind::IpHolding));
        assert_eq!(
            WhtBasis::parse("no_rate_on_file"),
            Some(WhtBasis::NoRateOnFile)
        );
        assert_eq!(Kind::parse("dividend"), None);
    }
}
