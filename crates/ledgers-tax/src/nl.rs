//! Dutch BTW (omzetbelasting): the aangifte draft, the ICP-opgaaf draft and
//! the KOR (kleineondernemersregeling) check.
//!
//! Pure functions over [`VatRecord`]s. Nothing here files a return: a draft is
//! for the owner to read, and filing is human-only (docs/PLAN.md section 5).
//!
//! Sources (checked 2026-10-23):
//! - Box labels 1a, 1b, 1e, 2a, 3b, 4a, 4b, 5b: RGS 3.7 MKB sheet, names of the
//!   BTW accounts (<https://www.referentiegrootboekschema.nl/definitieve-versie-rgs-37>).
//! - 5a (`Verschuldige btw (rubrieken 1 t/m 4)`) and 5b (`Voorbelasting`): the
//!   Belastingdienst page <https://www.belastingdienst.nl/wps/wcm/connect/nl/btw/content/ik-moet-btw-aangifte-doen-hoe-vul-ik-die-in>.
//! - Rounding of VAT amounts to whole euros in the taxpayer's favour:
//!   <https://www.belastingdienst.nl/wps/wcm/connect/bldcontentnl/belastingdienst/zakelijk/btw/administratie_bijhouden/facturen_maken/btw-bedrag_afronden>.
//! - KOR limit EUR 20,000 per calendar year, for the year and the year before:
//!   <https://www.belastingdienst.nl/wps/wcm/connect/nl/btw/content/kor-voorwaarden>.
//!
//! TODO (not mapped, not verified against a filed form): boxes 1c, 1d, 3a,
//! 3c, 5c and 5e-5g. Box 5b is taken to include the VAT on 2a, 4a and 4b
//! (reverse charge and import VAT are deductible there); confirm before filing.

use crate::settings::VatBasis;
use crate::validate::Validator;
use crate::vat_return::{ReturnClass, VatRecord, VatReturnError, in_period};
use ledgers_money::{Amount, Currency};
use std::collections::BTreeMap;
use time::Date;

/// The KOR turnover limit in euros per calendar year (`20.000`).
pub const KOR_LIMIT_EUR: i128 = 20_000;

/// Where a company stands against the KOR limit.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum KorStatus {
    /// The company has not chosen the KOR.
    NotChosen,
    /// Chosen, and turnover is within the limit.
    Within,
    /// Chosen, and turnover equals the limit: the next euro of turnover ends the KOR.
    AtLimit,
    /// Chosen, and turnover is above the limit: the KOR ends with the transaction
    /// that crossed it, and the company must deregister.
    Exceeded,
    /// Prior-year turnover is above the limit: the KOR does not apply this year.
    PriorYearExceeded,
}

/// The KOR status for a company, given its turnover this year and last year.
///
/// The turnover is the figure the Belastingdienst counts, which excludes
/// investment goods used in the business; the caller supplies that figure.
///
/// # Errors
/// [`VatReturnError::Money`] if the limit cannot be expressed in the currency.
pub fn kor_status(
    kor_chosen: bool,
    turnover_this_year: Amount,
    turnover_prior_year: Amount,
) -> Result<KorStatus, VatReturnError> {
    if !kor_chosen {
        return Ok(KorStatus::NotChosen);
    }
    let limit = Amount::from_major(KOR_LIMIT_EUR, turnover_this_year.currency())?;
    if turnover_prior_year.minor() > limit.minor() {
        return Ok(KorStatus::PriorYearExceeded);
    }
    Ok(match turnover_this_year.minor().cmp(&limit.minor()) {
        std::cmp::Ordering::Greater => KorStatus::Exceeded,
        std::cmp::Ordering::Equal => KorStatus::AtLimit,
        std::cmp::Ordering::Less => KorStatus::Within,
    })
}

/// One box of the aangifte draft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vakje {
    /// The box number (`1a`).
    pub rubriek: &'static str,
    /// The box name, from the RGS 3.7 account names.
    pub label: &'static str,
    /// Net value of the supplies or purchases in the box.
    pub omzet: Amount,
    /// VAT in the box, rounded to whole euros as the aangifte does.
    pub btw: Amount,
}

/// The BTW-aangifte draft for one period. Figures in EUR minor units.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BtwAangifteNl {
    /// First day of the period.
    pub from: Date,
    /// Last day of the period.
    pub to: Date,
    /// True when the KOR applies: no VAT is charged and no return is drafted,
    /// so every box is zero.
    pub kor: bool,
    /// Boxes 1a, 1b, 1e, 2a, 3b, 4a and 4b, in print order.
    pub rubrieken: Vec<Vakje>,
    /// Box 5a: VAT owed on rubrieken 1 to 4 (the sum of the rounded boxes).
    pub verschuldigde_btw: Amount,
    /// Box 5b: input VAT, rounded up to whole euros.
    pub voorbelasting: Amount,
    /// 5a minus 5b: positive is payable, negative is refundable. Boxes 5c and
    /// 5e to 5g are not mapped yet, so this is a draft balance only.
    pub saldo: Amount,
}

#[derive(Default, Clone, Copy)]
struct Sum {
    net: i128,
    vat: i128,
}

impl Sum {
    fn add(&mut self, net: Amount, vat: Amount) {
        self.net += net.minor();
        self.vat += vat.minor();
    }
}

/// Truncates a minor-unit VAT amount to whole euros. Used for VAT the taxpayer
/// owes (output boxes): rounding down favours the taxpayer.
fn round_down_to_euro(minor: i128) -> i128 {
    minor / 100 * 100
}

/// Rounds a minor-unit VAT amount up to whole euros. Used for the input VAT
/// the taxpayer deducts (box 5b): rounding up favours the taxpayer.
fn round_up_to_euro(minor: i128) -> i128 {
    (minor + 99).div_euclid(100) * 100
}

fn eur(minor: i128) -> Amount {
    Amount::new(
        minor,
        Currency::from_code("EUR").expect("EUR is a known currency"),
    )
}

/// The sums per box before rounding, in minor units.
#[derive(Default)]
struct Boxes {
    r1a: Sum,
    r1b: Sum,
    r1e: Sum,
    r2a: Sum,
    r3b: Sum,
    r4a: Sum,
    r4b: Sum,
    voorbelasting: i128,
}

/// Sorts the records of the period into boxes. Nothing is rounded here.
fn sum_boxes(records: &[VatRecord], from: Date, to: Date, basis: VatBasis) -> Boxes {
    let mut b = Boxes::default();
    for rec in records.iter().filter(|r| in_period(r, from, to, basis)) {
        let vat = rec.vat.minor();
        match &rec.class {
            ReturnClass::DomesticStandard => b.r1a.add(rec.net, rec.vat),
            ReturnClass::DomesticReduced => b.r1b.add(rec.net, rec.vat),
            ReturnClass::ZeroRated
            | ReturnClass::ExemptWithDeduction
            | ReturnClass::ExemptWithoutDeduction => b.r1e.net += rec.net.minor(),
            ReturnClass::ReverseChargeOther => {
                b.r2a.add(rec.net, rec.vat);
                b.voorbelasting += vat;
            }
            ReturnClass::IntraEuSupply { .. } | ReturnClass::IntraEuService { .. } => {
                b.r3b.net += rec.net.minor();
            }
            ReturnClass::ImportVat => {
                b.r4a.add(rec.net, rec.vat);
                b.voorbelasting += vat;
            }
            ReturnClass::IntraEuAcquisitionStandard
            | ReturnClass::IntraEuAcquisitionReduced
            | ReturnClass::ReverseChargeEuService => {
                b.r4b.add(rec.net, rec.vat);
                b.voorbelasting += vat;
            }
            ReturnClass::InputVat => b.voorbelasting += vat,
            ReturnClass::OssDistanceSale { .. } => {}
        }
    }
    b
}

/// Drafts the BTW-aangifte for `[from, to]` from the VAT records.
///
/// # Errors
/// [`VatReturnError::InvalidPeriod`] when `from > to`.
pub fn prepare_btw_aangifte_nl(
    records: &[VatRecord],
    from: Date,
    to: Date,
    basis: VatBasis,
    kor: bool,
) -> Result<BtwAangifteNl, VatReturnError> {
    if from > to {
        return Err(VatReturnError::InvalidPeriod);
    }
    let b = if kor {
        Boxes::default()
    } else {
        sum_boxes(records, from, to, basis)
    };
    let boxed = |rubriek: &'static str, label: &'static str, sum: Sum| Vakje {
        rubriek,
        label,
        omzet: eur(sum.net),
        btw: eur(round_down_to_euro(sum.vat)),
    };
    let rubrieken = vec![
        boxed(
            "1a",
            "Omzetbelasting leveringen/diensten belast met hoog tarief",
            b.r1a,
        ),
        boxed(
            "1b",
            "Omzetbelasting leveringen/diensten belast met laag tarief",
            b.r1b,
        ),
        Vakje {
            rubriek: "1e",
            label: "Leveringen/diensten belast met 0% of niet bij u belast",
            omzet: eur(b.r1e.net),
            btw: eur(0),
        },
        boxed(
            "2a",
            "Omzetbelasting leveringen/diensten waarbij de omzetbelasting naar u is verlegd",
            b.r2a,
        ),
        Vakje {
            rubriek: "3b",
            label: "Leveringen/diensten naar landen binnen de EU",
            omzet: eur(b.r3b.net),
            btw: eur(0),
        },
        boxed(
            "4a",
            "Omzetbelasting leveringen/diensten uit landen buiten de EU",
            b.r4a,
        ),
        boxed(
            "4b",
            "Omzetbelasting leveringen/diensten uit landen binnen de EU",
            b.r4b,
        ),
    ];
    let verschuldigde_btw: i128 = rubrieken.iter().map(|v| v.btw.minor()).sum();
    let voorbelasting = round_up_to_euro(b.voorbelasting);
    Ok(BtwAangifteNl {
        from,
        to,
        kor,
        rubrieken,
        verschuldigde_btw: eur(verschuldigde_btw),
        voorbelasting: eur(voorbelasting),
        saldo: eur(verschuldigde_btw - voorbelasting),
    })
}

/// Whether an ICP line is goods or services (the two ICP lines).
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum IcpSoort {
    /// Goods supplied to a customer in another member state.
    Goederen,
    /// Services supplied to a customer in another member state.
    Diensten,
}

/// One line of the ICP-opgaaf draft: one customer, one kind, the net total.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IcpRegel {
    /// The customer's EU VAT number.
    pub btw_id: String,
    /// Goods or services.
    pub soort: IcpSoort,
    /// Net value supplied in the period.
    pub omzet: Amount,
}

/// Drafts the ICP-opgaaf lines for `[from, to]`: intra-community supplies of
/// goods and services, grouped per customer VAT number.
///
/// TODO: the Belastingdienst ICP codes (per line) are not mapped here; the lines
/// carry goods or services only.
///
/// # Errors
/// [`VatReturnError::InvalidPeriod`] for `from > to`;
/// [`VatReturnError::InvalidVatId`] for a malformed customer number or a Dutch
/// one (a supply to a Dutch customer is not an intra-community supply).
pub fn prepare_icp_nl(
    records: &[VatRecord],
    from: Date,
    to: Date,
    basis: VatBasis,
) -> Result<Vec<IcpRegel>, VatReturnError> {
    if from > to {
        return Err(VatReturnError::InvalidPeriod);
    }
    let mut totals: BTreeMap<(String, IcpSoort), i128> = BTreeMap::new();
    for rec in records.iter().filter(|r| in_period(r, from, to, basis)) {
        let (vat_id, soort) = match &rec.class {
            ReturnClass::IntraEuSupply { vat_id } => (vat_id, IcpSoort::Goederen),
            ReturnClass::IntraEuService { vat_id } => (vat_id, IcpSoort::Diensten),
            _ => continue,
        };
        if vat_id.starts_with("NL") {
            return Err(VatReturnError::InvalidVatId(vat_id.clone()));
        }
        Validator::EuVat
            .check(vat_id)
            .map_err(|_| VatReturnError::InvalidVatId(vat_id.clone()))?;
        *totals.entry((vat_id.clone(), soort)).or_insert(0) += rec.net.minor();
    }
    Ok(totals
        .into_iter()
        .map(|((btw_id, soort), minor)| IcpRegel {
            btw_id,
            soort,
            omzet: eur(minor),
        })
        .collect())
}
