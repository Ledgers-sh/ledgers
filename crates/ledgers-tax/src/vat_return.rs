//! VAT return preparation: `UStVA`, EC Sales List, OSS.
//!
//! Pure functions for German VAT filing (2026). Mapping pending tax-adviser review.
//! Output is per-box figures for manual entry; filing is human-only.

use crate::settings::VatBasis;
use ledgers_money::{Amount, Currency, Rate, Rounding};
use std::collections::BTreeMap;
use thiserror::Error;
use time::Date;

/// VAT return classification (13 classes).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ReturnClass {
    DomesticStandard,
    DomesticReduced,
    IntraEuSupply { vat_id: String },
    IntraEuService { vat_id: String },
    ExemptWithDeduction,
    ExemptWithoutDeduction,
    IntraEuAcquisitionStandard,
    IntraEuAcquisitionReduced,
    ReverseChargeEuService,
    ReverseChargeOther,
    InputVat,
    ImportVat,
    OssDistanceSale { country: String, rate: Rate },
}

/// One VAT transaction record.
#[derive(Clone, Debug)]
pub struct VatRecord {
    pub tax_point: Date,
    pub paid_on: Option<Date>,
    pub class: ReturnClass,
    pub net: Amount,
    pub vat: Amount,
}

/// VAT return errors.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VatReturnError {
    #[error("unsupported tax year: {0}")]
    UnsupportedTaxYear(i32),
    #[error("currency must be EUR, got {0}")]
    Currency(String),
    #[error("invalid period: from > to")]
    InvalidPeriod,
    #[error("invalid VAT ID: {0}")]
    InvalidVatId(String),
    #[error(transparent)]
    Money(#[from] ledgers_money::MoneyError),
}

fn in_period(record: &VatRecord, from: Date, to: Date, basis: VatBasis) -> bool {
    let date = match basis {
        VatBasis::Accrual => record.tax_point,
        VatBasis::Cash => {
            let is_cash = matches!(
                record.class,
                ReturnClass::DomesticStandard
                    | ReturnClass::DomesticReduced
                    | ReturnClass::ExemptWithDeduction
                    | ReturnClass::ExemptWithoutDeduction
            );
            if is_cash {
                match record.paid_on {
                    Some(paid) => paid,
                    None => return false, // Unpaid records excluded from cash basis
                }
            } else {
                record.tax_point
            }
        }
    };
    date >= from && date <= to
}

fn to_whole_eur(minor: i128) -> i128 {
    (minor / 100) * 100
}

/// One `UStVA` box (Kz).
#[derive(Clone, Debug)]
pub struct UstvaBox {
    pub kz: String,
    pub label: String,
    pub amount: Amount,
}

/// Prepared `UStVA` return.
#[derive(Clone, Debug)]
pub struct UstvaReturn {
    pub tax_year: i32,
    pub from: Date,
    pub to: Date,
    pub basis: VatBasis,
    pub boxes: Vec<UstvaBox>,
}

impl UstvaReturn {
    #[must_use]
    pub fn to_csv(&self) -> String {
        let mut lines = vec!["kz,label,amount".to_string()];
        for b in &self.boxes {
            lines.push(format!(
                "{},{},{}",
                b.kz,
                b.label,
                b.amount.to_canonical_string()
            ));
        }
        lines.join("\n")
    }
}

/// Generic VAT/GST return.
#[derive(Clone, Debug)]
pub struct GenericReturn {
    pub taxable_sales: Amount,
    pub output_vat: Amount,
    pub zero_rated_and_exempt_sales: Amount,
    pub input_vat: Amount,
    pub net_payable: Amount,
}

/// EC Sales List row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EcSalesListRow {
    pub vat_id: String,
    pub country: String,
    pub kind: String,
    pub amount: Amount,
}

/// OSS row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OssRow {
    pub country: String,
    pub rate: Rate,
    pub net: Amount,
    pub vat: Amount,
}

/// Prepare `UStVA` return (2026, accrual or cash basis, base in whole EUR, tax rounded `HalfUp`).
///
/// # Panics
/// Panics if EUR currency code cannot be resolved.
///
/// # Errors
/// Returns `VatReturnError::InvalidPeriod` if from > to, `UnsupportedTaxYear` if not 2026 or spanning years,
/// `Currency` if records are not EUR, or `Money` on amount arithmetic error.
pub fn prepare_ustva(
    records: &[VatRecord],
    from: Date,
    to: Date,
    basis: VatBasis,
) -> Result<UstvaReturn, VatReturnError> {
    if from > to {
        return Err(VatReturnError::InvalidPeriod);
    }
    if from.year() != to.year() || from.year() != 2026 {
        return Err(VatReturnError::UnsupportedTaxYear(from.year()));
    }

    let eur = Currency::from_code("EUR").expect("EUR");
    let mut acc = UstvaAccumulator::new(eur);

    for rec in records {
        if !in_period(rec, from, to, basis) {
            continue;
        }
        if rec.net.currency().code() != "EUR" {
            return Err(VatReturnError::Currency(
                rec.net.currency().code().to_string(),
            ));
        }
        acc.add(rec);
    }

    let rate19 = Rate::parse("0.19")?;
    let rate7 = Rate::parse("0.07")?;
    acc.finalize(rate19, rate7, basis)
}

struct UstvaAccumulator {
    eur: Currency,
    kz81: i128,
    kz86: i128,
    kz41: i128,
    kz21: i128,
    kz43: i128,
    kz48: i128,
    kz89: i128,
    kz93: i128,
    kz46: i128,
    kz84: i128,
    kz47: i128,
    kz85: i128,
    kz66: i128,
    kz61: i128,
    kz62: i128,
    kz67: i128,
}

impl UstvaAccumulator {
    fn new(eur: Currency) -> Self {
        Self {
            eur,
            kz81: 0,
            kz86: 0,
            kz41: 0,
            kz21: 0,
            kz43: 0,
            kz48: 0,
            kz89: 0,
            kz93: 0,
            kz46: 0,
            kz84: 0,
            kz47: 0,
            kz85: 0,
            kz66: 0,
            kz61: 0,
            kz62: 0,
            kz67: 0,
        }
    }

    fn add(&mut self, rec: &VatRecord) {
        match &rec.class {
            ReturnClass::DomesticStandard => self.kz81 += rec.net.minor(),
            ReturnClass::DomesticReduced => self.kz86 += rec.net.minor(),
            ReturnClass::IntraEuSupply { .. } => self.kz41 += rec.net.minor(),
            ReturnClass::IntraEuService { .. } => self.kz21 += rec.net.minor(),
            ReturnClass::ExemptWithDeduction => self.kz43 += rec.net.minor(),
            ReturnClass::ExemptWithoutDeduction => self.kz48 += rec.net.minor(),
            ReturnClass::IntraEuAcquisitionStandard => {
                self.kz89 += rec.net.minor();
                self.kz61 += rec.vat.minor();
            }
            ReturnClass::IntraEuAcquisitionReduced => {
                self.kz93 += rec.net.minor();
                self.kz61 += rec.vat.minor();
            }
            ReturnClass::ReverseChargeEuService => {
                self.kz46 += rec.net.minor();
                self.kz47 += rec.vat.minor();
                self.kz67 += rec.vat.minor();
            }
            ReturnClass::ReverseChargeOther => {
                self.kz84 += rec.net.minor();
                self.kz85 += rec.vat.minor();
                self.kz67 += rec.vat.minor();
            }
            ReturnClass::InputVat => self.kz66 += rec.vat.minor(),
            ReturnClass::ImportVat => self.kz62 += rec.vat.minor(),
            ReturnClass::OssDistanceSale { .. } => {}
        }
    }

    #[allow(clippy::too_many_lines)]
    fn finalize(
        self,
        rate19: Rate,
        rate7: Rate,
        basis: VatBasis,
    ) -> Result<UstvaReturn, VatReturnError> {
        let b = |kz: &str, label: &str, amt: i128| UstvaBox {
            kz: kz.to_string(),
            label: label.to_string(),
            amount: Amount::new(amt, self.eur),
        };

        let mut boxes = Vec::new();
        let tax_81 = if self.kz81 != 0 {
            let whole = Amount::new(to_whole_eur(self.kz81), self.eur);
            let t = whole.mul_rate(rate19, Rounding::HalfUp)?;
            boxes.push(b(
                "81",
                "Steuerpflichtige Umsätze (19%)",
                to_whole_eur(self.kz81),
            ));
            boxes.push(b("81-tax", "Umsatzsteuer (19%)", t.minor()));
            t.minor()
        } else {
            0
        };

        let tax_86 = if self.kz86 != 0 {
            let whole = Amount::new(to_whole_eur(self.kz86), self.eur);
            let t = whole.mul_rate(rate7, Rounding::HalfUp)?;
            boxes.push(b(
                "86",
                "Steuerpflichtige Umsätze (7%)",
                to_whole_eur(self.kz86),
            ));
            boxes.push(b("86-tax", "Umsatzsteuer (7%)", t.minor()));
            t.minor()
        } else {
            0
        };

        if self.kz41 != 0 {
            boxes.push(b(
                "41",
                "Lieferungen ins EU-Ausland",
                to_whole_eur(self.kz41),
            ));
        }
        if self.kz21 != 0 {
            boxes.push(b("21", "Sonstige Leistungen", to_whole_eur(self.kz21)));
        }
        if self.kz43 != 0 {
            boxes.push(b(
                "43",
                "Umsätze mit Vorsteuerabzug",
                to_whole_eur(self.kz43),
            ));
        }
        if self.kz48 != 0 {
            boxes.push(b(
                "48",
                "Umsätze ohne Vorsteuerabzug",
                to_whole_eur(self.kz48),
            ));
        }

        let tax_89 = if self.kz89 != 0 {
            let whole = Amount::new(to_whole_eur(self.kz89), self.eur);
            let t = whole.mul_rate(rate19, Rounding::HalfUp)?;
            boxes.push(b("89", "Intra-EU-Erwerbe (19%)", to_whole_eur(self.kz89)));
            boxes.push(b("89-tax", "Umsatzsteuer (19%)", t.minor()));
            t.minor()
        } else {
            0
        };

        let tax_93 = if self.kz93 != 0 {
            let whole = Amount::new(to_whole_eur(self.kz93), self.eur);
            let t = whole.mul_rate(rate7, Rounding::HalfUp)?;
            boxes.push(b("93", "Intra-EU-Erwerbe (7%)", to_whole_eur(self.kz93)));
            boxes.push(b("93-tax", "Umsatzsteuer (7%)", t.minor()));
            t.minor()
        } else {
            0
        };

        if self.kz46 != 0 {
            boxes.push(b(
                "46",
                "Reverse Charge EU-Dienste",
                to_whole_eur(self.kz46),
            ));
        }
        if self.kz47 != 0 {
            boxes.push(b("47", "Reverse Charge (Steuer)", self.kz47));
        }
        if self.kz84 != 0 {
            boxes.push(b("84", "Reverse Charge sonstiges", to_whole_eur(self.kz84)));
        }
        if self.kz85 != 0 {
            boxes.push(b("85", "Reverse Charge (Steuer)", self.kz85));
        }
        if self.kz66 != 0 {
            boxes.push(b("66", "Vorsteuer (Inland)", self.kz66));
        }
        if self.kz61 != 0 {
            boxes.push(b("61", "Vorsteuer (Intra-EU)", self.kz61));
        }
        if self.kz62 != 0 {
            boxes.push(b("62", "Vorsteuer (Einfuhr)", self.kz62));
        }
        if self.kz67 != 0 {
            boxes.push(b("67", "Vorsteuer (Reverse Charge)", self.kz67));
        }

        let kz83 = tax_81 + tax_86 + tax_89 + tax_93 + self.kz47 + self.kz85
            - (self.kz66 + self.kz61 + self.kz62 + self.kz67);
        boxes.push(b(
            "83",
            "Verbleibende Umsatzsteuer-Vorauszahlung bzw. Überschuss",
            kz83,
        ));
        Ok(UstvaReturn {
            tax_year: 2026,
            from: Date::from_calendar_date(2026, time::Month::January, 1).unwrap(),
            to: Date::from_calendar_date(2026, time::Month::December, 31).unwrap(),
            basis,
            boxes,
        })
    }
}

/// Generic return (net payable = output − input).
///
/// # Panics
/// Panics if EUR currency code cannot be resolved.
///
/// # Errors
/// Returns `Money` on amount arithmetic error.
pub fn prepare_generic_return(
    records: &[VatRecord],
    from: Date,
    to: Date,
    basis: VatBasis,
) -> Result<GenericReturn, VatReturnError> {
    let eur = Currency::from_code("EUR").expect("EUR");
    let mut taxable = Amount::new(0, eur);
    let mut output = Amount::new(0, eur);
    let mut exempt = Amount::new(0, eur);
    let mut input = Amount::new(0, eur);

    for rec in records {
        if !in_period(rec, from, to, basis) {
            continue;
        }

        match &rec.class {
            ReturnClass::DomesticStandard | ReturnClass::DomesticReduced => {
                taxable = taxable.checked_add(rec.net)?;
                output = output.checked_add(rec.vat)?;
            }
            ReturnClass::IntraEuSupply { .. }
            | ReturnClass::IntraEuService { .. }
            | ReturnClass::ExemptWithDeduction
            | ReturnClass::ExemptWithoutDeduction => {
                exempt = exempt.checked_add(rec.net)?;
            }
            ReturnClass::ReverseChargeEuService | ReturnClass::ReverseChargeOther => {
                output = output.checked_add(rec.vat)?;
                input = input.checked_add(rec.vat)?;
            }
            ReturnClass::InputVat
            | ReturnClass::ImportVat
            | ReturnClass::IntraEuAcquisitionStandard
            | ReturnClass::IntraEuAcquisitionReduced => {
                input = input.checked_add(rec.vat)?;
            }
            ReturnClass::OssDistanceSale { .. } => {}
        }
    }

    Ok(GenericReturn {
        taxable_sales: taxable,
        output_vat: output,
        zero_rated_and_exempt_sales: exempt,
        input_vat: input,
        net_payable: output.checked_sub(input)?,
    })
}

/// EC Sales List (Zusammenfassende Meldung).
///
/// # Panics
/// Panics if EUR currency code cannot be resolved.
///
/// # Errors
/// Returns `InvalidVatId` if a VAT ID is malformed or is "DE".
pub fn prepare_ec_sales_list(
    records: &[VatRecord],
    from: Date,
    to: Date,
) -> Result<Vec<EcSalesListRow>, VatReturnError> {
    let eur = Currency::from_code("EUR").expect("EUR");
    let mut grouped: BTreeMap<(String, String), i128> = BTreeMap::new();

    for rec in records {
        if rec.tax_point < from || rec.tax_point > to {
            continue;
        }

        match &rec.class {
            ReturnClass::IntraEuSupply { vat_id } => {
                let id = normalize_vat_id(vat_id)?;
                *grouped.entry((id, "L".to_string())).or_insert(0) += rec.net.minor();
            }
            ReturnClass::IntraEuService { vat_id } => {
                let id = normalize_vat_id(vat_id)?;
                *grouped.entry((id, "S".to_string())).or_insert(0) += rec.net.minor();
            }
            _ => {}
        }
    }

    let mut rows = Vec::new();
    for ((vat_id, kind), sum) in grouped {
        let country = extract_country(&vat_id)?;
        rows.push(EcSalesListRow {
            vat_id,
            country,
            kind,
            amount: Amount::new((sum / 100) * 100, eur),
        });
    }
    rows.sort_by(|a, b| a.vat_id.cmp(&b.vat_id).then_with(|| a.kind.cmp(&b.kind)));
    Ok(rows)
}

/// OSS (One-Stop Shop).
///
/// # Panics
/// Panics if EUR currency code cannot be resolved.
///
/// # Errors
/// Returns `InvalidVatId` if a country code is not 2 letters or is "DE".
pub fn prepare_oss(
    records: &[VatRecord],
    from: Date,
    to: Date,
) -> Result<Vec<OssRow>, VatReturnError> {
    let eur = Currency::from_code("EUR").expect("EUR");
    let mut grouped: BTreeMap<(String, i128), (i128, i128)> = BTreeMap::new();

    for rec in records {
        if rec.tax_point < from || rec.tax_point > to {
            continue;
        }

        if let ReturnClass::OssDistanceSale { country, rate } = &rec.class {
            let c = country.to_uppercase();
            if c.len() != 2 {
                return Err(VatReturnError::InvalidVatId(format!("OSS country: {c}")));
            }
            if c == "DE" {
                return Err(VatReturnError::InvalidVatId("OSS country DE".to_string()));
            }
            let e = grouped.entry((c, rate.scaled())).or_insert((0, 0));
            e.0 += rec.net.minor();
            e.1 += rec.vat.minor();
        }
    }

    let mut rows = Vec::new();
    for ((country, scaled), (net, vat)) in grouped {
        rows.push(OssRow {
            country,
            rate: Rate::from_scaled(scaled),
            net: Amount::new(net, eur),
            vat: Amount::new(vat, eur),
        });
    }
    rows.sort_by(|a, b| {
        a.country
            .cmp(&b.country)
            .then_with(|| a.rate.scaled().cmp(&b.rate.scaled()))
    });
    Ok(rows)
}

fn normalize_vat_id(id: &str) -> Result<String, VatReturnError> {
    let n = id.to_uppercase().replace(' ', "");
    if n.len() < 3 || !n.chars().take(2).all(|c| c.is_ascii_alphabetic()) {
        return Err(VatReturnError::InvalidVatId(id.to_string()));
    }
    if n.starts_with("DE") {
        return Err(VatReturnError::InvalidVatId("DE VAT ID".to_string()));
    }
    Ok(n)
}

fn extract_country(vat_id: &str) -> Result<String, VatReturnError> {
    if vat_id.len() < 2 {
        return Err(VatReturnError::InvalidVatId(vat_id.to_string()));
    }
    Ok(vat_id[..2].to_uppercase())
}
