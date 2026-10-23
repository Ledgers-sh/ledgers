//! Country packs: the tax regime of one country (or group of countries), as
//! data.
//!
//! A pack is `templates/countries/<id>/pack.toml`. It names the chart the
//! company starts from, the regime (VAT, GST, sales tax or none), the dated
//! statutory rates with their sources, the registration-number validator, the
//! tax codes entries use, and the return layouts the core can draft. The core
//! reads packs; it never names a country. Each country is one pack (the Singapore GST pack
//! is one example), and the generic pack has no statutory rates at all:
//! the company enters its own (see [`CountryPack::rate_table`]).
//!
//! Packs are compiled in with `include_str!`, so the same bytes run on the
//! native binary and on the Worker.

use crate::calc::RoundingScope;
use crate::code::{TaxCode, TaxKind};
use crate::error::TaxError;
use crate::rate::{DatedRate, Jurisdiction, RateCategory, RateTable};
use crate::validate::Validator;
use ledgers_money::{Currency, Rate};
use serde::Deserialize;
use std::collections::BTreeMap;
use time::Date;

/// The tax regime a pack describes. Drives the wording and the return layout,
/// not the arithmetic (which is the same for every regime).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Regime {
    /// Value-added tax (EU, UK, and others).
    Vat,
    /// Goods and services tax (Singapore, Australia).
    Gst,
    /// A tax added at the point of sale, levied by jurisdiction (US states).
    SalesTax,
    /// No indirect tax on sales.
    None,
}

impl Regime {
    /// The regime named in a pack file.
    ///
    /// # Errors
    /// [`TaxError::Invalid`] for an unknown name.
    pub fn from_code(code: &str) -> Result<Self, TaxError> {
        Ok(match code {
            "vat" => Regime::Vat,
            "gst" => Regime::Gst,
            "sales_tax" => Regime::SalesTax,
            "none" => Regime::None,
            other => {
                return Err(TaxError::Invalid {
                    reason: format!("unknown tax regime {other:?}"),
                });
            }
        })
    }

    /// The code a pack file uses for this regime.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Regime::Vat => "vat",
            Regime::Gst => "gst",
            Regime::SalesTax => "sales_tax",
            Regime::None => "none",
        }
    }
}

/// How a pack writes numbers, for the number and date defaults.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct NumberFormat {
    /// The decimal separator, `.` or `,`.
    pub decimal: char,
    /// The thousands separator.
    pub thousands: char,
}

/// The registration number a company gives on invoices and returns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registration {
    /// The name of the number, as the authority calls it (`UEN`, `NPWP`, `BTW-id`).
    pub label: String,
    /// The check that runs when the company saves the number.
    pub validator: Validator,
    /// What the check covers and what it does not, from the pack file.
    pub note: String,
}

/// A tax code a pack defines, before its accounts are known.
///
/// The output and input accounts are named by chart tag (`vat-output`,
/// `vat-input`), so the same code works on any chart that carries the tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackTaxCode {
    /// The stable id entries and bills store (`SG-GST-SR-9`).
    pub code: String,
    /// The English label; the app translates by `code`.
    pub label: String,
    /// What the code is for.
    pub kind: TaxKind,
    /// The rate category the code uses.
    pub category: RateCategory,
    /// The chart tag of the output account, when the kind has one.
    pub output_tag: Option<String>,
    /// The chart tag of the input account, when the kind has one.
    pub input_tag: Option<String>,
}

/// One figure a return box can show, taken from a generic return.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Figure {
    /// Net value of standard-rated sales.
    StandardNet,
    /// VAT on standard-rated sales.
    StandardVat,
    /// Net value of reduced-rated sales.
    ReducedNet,
    /// VAT on reduced-rated sales.
    ReducedVat,
    /// Net value of zero-rated sales.
    ZeroRatedNet,
    /// Net value of exempt sales (and intra-EU supplies).
    ExemptNet,
    /// Output VAT in total, including reverse-charge VAT.
    OutputVat,
    /// Input VAT in total, including reverse-charge VAT.
    InputVat,
    /// Output minus input VAT: positive is payable, negative is refundable.
    NetPayable,
}

impl Figure {
    /// The figure a pack file names.
    ///
    /// # Errors
    /// [`TaxError::Invalid`] for an unknown name.
    pub fn from_code(code: &str) -> Result<Self, TaxError> {
        Ok(match code {
            "standard_net" => Figure::StandardNet,
            "standard_vat" => Figure::StandardVat,
            "reduced_net" => Figure::ReducedNet,
            "reduced_vat" => Figure::ReducedVat,
            "zero_rated_net" => Figure::ZeroRatedNet,
            "exempt_net" => Figure::ExemptNet,
            "output_vat" => Figure::OutputVat,
            "input_vat" => Figure::InputVat,
            "net_payable" => Figure::NetPayable,
            other => {
                return Err(TaxError::Invalid {
                    reason: format!("unknown return figure {other:?}"),
                });
            }
        })
    }
}

/// One numbered box of a country return layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReturnBox {
    /// The box number as the form prints it (`1`, `6`, `1a`).
    pub number: String,
    /// The English label of the box.
    pub label: String,
    /// Where the figure comes from.
    pub figure: Figure,
}

/// A return the pack can draft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReturnLayout {
    /// The generic VAT/GST return: output, input and net per period, by rate.
    /// Every pack has it.
    Generic,
    /// The `UStVA` (Umsatzsteuer-Voranmeldung) layout, built by [`crate::vat_return::prepare_ustva`].
    Ustva,
    /// A layout of numbered boxes over generic figures (for example the
    /// Singapore GST F5 or the Dutch aangifte).
    Boxes {
        /// The layout id (`sg-gst-f5`).
        id: String,
        /// The English title of the form.
        label: String,
        /// The boxes, in print order.
        boxes: Vec<ReturnBox>,
    },
}

/// A source-cited statutory rate set, with its dates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CountryPack {
    /// The pack id, the folder name (`sg`, `eu`, `generic`).
    pub id: String,
    /// The primary ISO 3166-1 alpha-2 code (`SG`), or `EU` for the EU pack.
    pub iso: String,
    /// The countries this pack applies to, in pack order. Empty for the generic pack.
    pub covers: Vec<String>,
    /// The English name of the country or group.
    pub name: String,
    /// The name in the country's own language.
    pub native_name: String,
    /// The currency the statutory returns are reported in. `None` means the
    /// company's base currency (the generic pack, where the company decides).
    pub currency: Option<Currency>,
    /// The first month of the default fiscal year (1 = January).
    pub fiscal_year_start_month: u8,
    /// Number separators for display.
    pub number_format: NumberFormat,
    /// The display order of day, month and year (`DD/MM/YYYY`).
    pub date_format: String,
    /// The chart template the company starts from (`generic-sme`, `de-hgb`).
    pub chart: String,
    /// The regime.
    pub regime: Regime,
    /// Whether tax is rounded per line or per document.
    pub rounding: RoundingScope,
    /// True when the company enters its own rates (generic and US packs).
    pub user_rates: bool,
    /// True while the pack lists rates still unverified (a TODO in the file).
    pub rates_pending: bool,
    /// The registration number the company gives.
    pub registration: Registration,
    /// The statutory rates, each with a valid-from date and a source.
    pub rates: Vec<DatedRate>,
    /// The tax codes entries use.
    pub tax_codes: Vec<PackTaxCode>,
    /// The return layouts the pack can draft.
    pub returns: Vec<ReturnLayout>,
    /// Withholding tax on payments abroad (advisory; see [`Withholding`]).
    pub withholding: Vec<Withholding>,
    /// Advisory notes with their sources (never tax advice; see [`CountryNote`]).
    pub notes: Vec<CountryNote>,
}

/// A withholding tax rate on a kind of payment a resident makes to a
/// non-resident. Used by intercompany agreements (ledgers-group) as the
/// default rate, which a treaty override can replace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Withholding {
    /// The payment kind: `royalty` is the only kind the packs carry today.
    pub kind: String,
    /// The statutory rate in basis points (`1000` = 10%). `None` when the
    /// rate is not verified: the pack then records the question, not a rate.
    pub rate_bp: Option<i64>,
    /// The official source the rate was read from.
    pub source: String,
    /// True when the rate was checked against the official source.
    pub verified: bool,
    /// Who the rate applies to and what it does not cover.
    pub note: String,
}

/// An advisory note for a country: a regime or incentive the company may
/// check. It is information with a source, not tax advice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CountryNote {
    /// A short topic name, for example `innovatiebox`.
    pub topic: String,
    /// The note text.
    pub text: String,
    /// The official source.
    pub source: String,
    /// True when the note was checked against the official source.
    pub verified: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPack {
    id: String,
    iso: String,
    #[serde(default)]
    covers: Vec<String>,
    name: String,
    native_name: String,
    currency: String,
    #[serde(default = "calendar_year")]
    fiscal_year_start_month: u8,
    decimal_separator: String,
    thousands_separator: String,
    date_format: String,
    chart: String,
    regime: String,
    rounding: String,
    #[serde(default)]
    user_rates: bool,
    /// True while some statutory rates are still unverified (listed as TODO
    /// in the pack file). The pack then carries only the rates it has sources for.
    #[serde(default)]
    rates_pending: bool,
    registration: RawRegistration,
    #[serde(default)]
    rates: Vec<RawRate>,
    #[serde(default)]
    tax_codes: Vec<RawTaxCode>,
    #[serde(default)]
    returns: Vec<RawReturn>,
    #[serde(default)]
    withholding: Vec<RawWithholding>,
    #[serde(default)]
    notes: Vec<RawNote>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWithholding {
    kind: String,
    /// A percentage text (`"10"`); absent while the rate is unverified.
    percent: Option<String>,
    source: String,
    verified: bool,
    note: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawNote {
    topic: String,
    text: String,
    source: String,
    verified: bool,
}

fn calendar_year() -> u8 {
    1
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRegistration {
    label: String,
    validator: String,
    note: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRate {
    jurisdiction: String,
    category: String,
    percent: String,
    valid_from: String,
    valid_to: Option<String>,
    source: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTaxCode {
    code: String,
    label: String,
    kind: String,
    category: String,
    output_tag: Option<String>,
    input_tag: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawReturn {
    id: String,
    label: String,
    kind: String,
    #[serde(default)]
    boxes: Vec<RawBox>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBox {
    number: String,
    label: String,
    figure: String,
}

fn invalid(reason: impl Into<String>) -> TaxError {
    TaxError::Invalid {
        reason: reason.into(),
    }
}

/// Converts a percentage text (`"9"`, `"5.5"`) into an exact decimal fraction
/// (`"0.09"`, `"0.055"`) without floats.
fn percent_to_fraction(percent: &str) -> Result<String, TaxError> {
    let (whole, frac) = percent.split_once('.').unwrap_or((percent, ""));
    if whole.is_empty() && frac.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !frac.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(invalid(format!("rate {percent:?} is not a percentage")));
    }
    let digits = format!("{whole}{frac}");
    let places = frac.len() + 2;
    let padded = format!("{digits:0>width$}", width = places + 1);
    let split = padded.len() - places;
    let text = format!("{}.{}", &padded[..split], &padded[split..]);
    let trimmed = text.trim_end_matches('0');
    Ok(trimmed.strip_suffix('.').unwrap_or(trimmed).to_owned())
}

/// A percentage text (`"19"`, `"5.5"`) as an exact [`Rate`].
///
/// # Errors
/// [`TaxError::Invalid`] when the text is not a plain percentage.
pub fn percent_rate(percent: &str) -> Result<Rate, TaxError> {
    let fraction = percent_to_fraction(percent)?;
    fraction
        .parse()
        .map_err(|e: ledgers_money::MoneyError| invalid(format!("rate {percent}: {e}")))
}

/// A `YYYY-MM-DD` date.
///
/// # Errors
/// [`TaxError::Invalid`] for any other text.
pub fn parse_date(text: &str) -> Result<Date, TaxError> {
    let bad = || invalid(format!("{text:?} is not a YYYY-MM-DD date"));
    let mut parts = text.split('-');
    let (Some(y), Some(m), Some(d), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(bad());
    };
    let year: i32 = y.parse().map_err(|_| bad())?;
    let month: u8 = m.parse().map_err(|_| bad())?;
    let day: u8 = d.parse().map_err(|_| bad())?;
    let month = time::Month::try_from(month).map_err(|_| bad())?;
    Date::from_calendar_date(year, month, day).map_err(|_| bad())
}

fn parse_char(text: &str, what: &str) -> Result<char, TaxError> {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Ok(c),
        _ => Err(invalid(format!("{what} must be one character"))),
    }
}

fn parse_category(text: &str) -> Result<RateCategory, TaxError> {
    RateCategory::from_code(text)
}

fn parse_tax_codes(pack_id: &str, raw: &[RawTaxCode]) -> Result<Vec<PackTaxCode>, TaxError> {
    let mut seen_codes = std::collections::BTreeSet::new();
    let mut tax_codes = Vec::with_capacity(raw.len());
    for code in raw {
        if !seen_codes.insert(code.code.clone()) {
            return Err(invalid(format!(
                "pack {pack_id}: duplicate tax code {}",
                code.code
            )));
        }
        tax_codes.push(PackTaxCode {
            code: code.code.clone(),
            label: code.label.clone(),
            kind: TaxKind::from_code(&code.kind)?,
            category: parse_category(&code.category)?,
            output_tag: code.output_tag.clone(),
            input_tag: code.input_tag.clone(),
        });
    }
    Ok(tax_codes)
}

fn parse_returns(raw: &[RawReturn]) -> Result<Vec<ReturnLayout>, TaxError> {
    let mut returns = Vec::with_capacity(raw.len());
    for ret in raw {
        returns.push(match ret.kind.as_str() {
            "generic" => ReturnLayout::Generic,
            "ustva" => ReturnLayout::Ustva,
            "boxes" => {
                let mut boxes = Vec::with_capacity(ret.boxes.len());
                for b in &ret.boxes {
                    boxes.push(ReturnBox {
                        number: b.number.clone(),
                        label: b.label.clone(),
                        figure: Figure::from_code(&b.figure)?,
                    });
                }
                ReturnLayout::Boxes {
                    id: ret.id.clone(),
                    label: ret.label.clone(),
                    boxes,
                }
            }
            other => return Err(invalid(format!("unknown return kind {other:?}"))),
        });
    }
    Ok(returns)
}

fn parse_rates(
    pack_id: &str,
    covers: &[String],
    raw: &[RawRate],
) -> Result<Vec<DatedRate>, TaxError> {
    let mut rates = Vec::with_capacity(raw.len());
    for rate in raw {
        if rate.source.trim().is_empty() {
            return Err(invalid(format!(
                "pack {}: every rate needs a source ({} {})",
                pack_id, rate.jurisdiction, rate.category
            )));
        }
        let jurisdiction = Jurisdiction::new(&rate.jurisdiction);
        if !covers.is_empty() && !covers.iter().any(|c| c == jurisdiction.code()) {
            return Err(invalid(format!(
                "pack {}: rate jurisdiction {} is not covered by this pack",
                pack_id,
                jurisdiction.code()
            )));
        }
        let fraction = percent_to_fraction(&rate.percent)?;
        let parsed: Rate = fraction.parse().map_err(|e: ledgers_money::MoneyError| {
            invalid(format!("pack {}: rate {}: {e}", pack_id, rate.percent))
        })?;
        rates.push(DatedRate {
            jurisdiction,
            category: parse_category(&rate.category)?,
            rate: parsed,
            valid_from: parse_date(&rate.valid_from)?,
            valid_to: rate.valid_to.as_deref().map(parse_date).transpose()?,
            source: rate.source.clone(),
        });
    }
    Ok(rates)
}

impl CountryPack {
    /// Parses and validates one pack file.
    ///
    /// # Errors
    /// [`TaxError::Invalid`] naming the first key that does not fit, or
    /// [`TaxError::RateTableInvalid`] when a rate sequence has gaps or overlaps.
    pub fn parse(text: &str) -> Result<Self, TaxError> {
        let raw: RawPack = toml::from_str(text).map_err(|e| invalid(e.to_string()))?;
        Self::from_raw(raw)
    }

    fn from_raw(raw: RawPack) -> Result<Self, TaxError> {
        let iso_ok = raw.iso.len() == 2 && raw.iso.bytes().all(|b| b.is_ascii_uppercase());
        if !iso_ok && raw.iso != "EU" {
            return Err(invalid(format!(
                "pack {}: iso must be two capitals or EU",
                raw.id
            )));
        }
        let covers: Vec<String> = raw.covers.iter().map(|c| c.to_ascii_uppercase()).collect();
        let currency = if raw.currency.is_empty() {
            None
        } else {
            Some(
                Currency::from_code(&raw.currency)
                    .map_err(|e| invalid(format!("pack {}: currency: {e}", raw.id)))?,
            )
        };
        if !(1..=12).contains(&raw.fiscal_year_start_month) {
            return Err(invalid(format!(
                "pack {}: fiscal_year_start_month is 1 to 12",
                raw.id
            )));
        }
        let regime = Regime::from_code(&raw.regime)?;
        let rounding = match raw.rounding.as_str() {
            "per_line" => RoundingScope::PerLine,
            "per_document" => RoundingScope::PerDocument,
            other => return Err(invalid(format!("unknown rounding scope {other:?}"))),
        };
        let validator = Validator::from_code(&raw.registration.validator)?;

        let rates = parse_rates(&raw.id, &covers, &raw.rates)?;
        // Gaps and overlaps per jurisdiction and category.
        RateTable::new(rates.clone())?;

        let tax_codes = parse_tax_codes(&raw.id, &raw.tax_codes)?;
        let mut returns = parse_returns(&raw.returns)?;
        // The generic return is in every pack, so it is added when the file leaves it out.
        if !returns.iter().any(|r| matches!(r, ReturnLayout::Generic)) {
            returns.insert(0, ReturnLayout::Generic);
        }

        let has_standard = rates.iter().any(|r| r.category == RateCategory::Standard);
        if matches!(regime, Regime::Vat | Regime::Gst)
            && !has_standard
            && !raw.user_rates
            && !raw.rates_pending
        {
            return Err(invalid(format!(
                "pack {}: a VAT or GST regime needs a standard rate, user_rates = true, or rates_pending = true",
                raw.id
            )));
        }
        if regime == Regime::None && (!rates.is_empty() || raw.user_rates) {
            return Err(invalid(format!(
                "pack {}: regime none carries no rates",
                raw.id
            )));
        }

        let withholding = raw
            .withholding
            .iter()
            .map(|w| parse_withholding(&raw.id, w))
            .collect::<Result<Vec<_>, _>>()?;
        let notes = raw
            .notes
            .into_iter()
            .map(|n| CountryNote {
                topic: n.topic,
                text: n.text,
                source: n.source,
                verified: n.verified,
            })
            .collect();

        Ok(CountryPack {
            id: raw.id,
            iso: raw.iso,
            covers,
            name: raw.name,
            native_name: raw.native_name,
            currency,
            fiscal_year_start_month: raw.fiscal_year_start_month,
            number_format: NumberFormat {
                decimal: parse_char(&raw.decimal_separator, "decimal_separator")?,
                thousands: parse_char(&raw.thousands_separator, "thousands_separator")?,
            },
            date_format: raw.date_format,
            chart: raw.chart,
            regime,
            rounding,
            user_rates: raw.user_rates,
            rates_pending: raw.rates_pending,
            registration: Registration {
                label: raw.registration.label,
                validator,
                note: raw.registration.note,
            },
            rates,
            tax_codes,
            returns,
            withholding,
            notes,
        })
    }

    /// True when this pack applies to the country (ISO alpha-2, any case).
    #[must_use]
    pub fn covers_country(&self, country: &str) -> bool {
        let code = country.trim().to_ascii_uppercase();
        self.covers.contains(&code)
    }

    /// The rate table: the pack's statutory rates, plus the company's own
    /// rates when the pack allows them.
    ///
    /// # Errors
    /// [`TaxError::RateTableInvalid`] when the combined rates overlap or leave a gap.
    pub fn rate_table(&self, company_rates: &[DatedRate]) -> Result<RateTable, TaxError> {
        let mut rates = self.rates.clone();
        if self.user_rates {
            rates.extend(company_rates.iter().cloned());
        }
        RateTable::new(rates)
    }

    /// The rate of a category in force on a day, for one jurisdiction.
    ///
    /// # Errors
    /// [`TaxError::RateNotFound`] when no rate covers the day.
    pub fn rate_on<'a>(
        table: &'a RateTable,
        jurisdiction: &Jurisdiction,
        category: RateCategory,
        date: Date,
    ) -> Result<&'a DatedRate, TaxError> {
        table.rate_on(jurisdiction, category, date)
    }

    /// The tax codes with their accounts resolved from chart tags.
    ///
    /// # Errors
    /// [`TaxError::MissingAccount`] when a code needs an account whose tag the
    /// chart does not carry, or another [`TaxCode::new`] error.
    pub fn tax_codes(
        &self,
        accounts_by_tag: &BTreeMap<String, String>,
    ) -> Result<Vec<TaxCode>, TaxError> {
        let lookup =
            |tag: &Option<String>| -> Result<Option<String>, TaxError> {
                match tag {
                    None => Ok(None),
                    Some(tag) => accounts_by_tag.get(tag).cloned().map(Some).ok_or_else(|| {
                        TaxError::Invalid {
                            reason: format!("chart has no account tagged {tag:?}"),
                        }
                    }),
                }
            };
        self.tax_codes
            .iter()
            .map(|code| {
                TaxCode::new(
                    code.code.clone(),
                    code.kind,
                    code.category,
                    lookup(&code.output_tag)?,
                    lookup(&code.input_tag)?,
                    Rate::ZERO,
                )
            })
            .collect()
    }
}

/// The bundled packs, as `(folder id, file text)`. Order matters for
/// [`pack_for_country`]: the first pack that covers a country wins, so a
/// country-specific pack sits before the EU group and the generic pack.
const BUNDLED: &[(&str, &str)] = &[
    (
        "de",
        include_str!("../../../templates/countries/de/pack.toml"),
    ),
    (
        "sg",
        include_str!("../../../templates/countries/sg/pack.toml"),
    ),
    (
        "id",
        include_str!("../../../templates/countries/id/pack.toml"),
    ),
    (
        "nl",
        include_str!("../../../templates/countries/nl/pack.toml"),
    ),
    (
        "gb",
        include_str!("../../../templates/countries/gb/pack.toml"),
    ),
    (
        "au",
        include_str!("../../../templates/countries/au/pack.toml"),
    ),
    (
        "us",
        include_str!("../../../templates/countries/us/pack.toml"),
    ),
    (
        "eu",
        include_str!("../../../templates/countries/eu/pack.toml"),
    ),
    (
        "generic",
        include_str!("../../../templates/countries/generic/pack.toml"),
    ),
];

/// Every bundled pack, parsed and validated.
///
/// # Errors
/// The first pack file that fails [`CountryPack::parse`].
/// A withholding entry: a percentage becomes basis points. A verified entry
/// must carry a rate or say why it has none in its note.
fn parse_withholding(pack: &str, raw: &RawWithholding) -> Result<Withholding, TaxError> {
    if raw.kind.is_empty() || raw.source.is_empty() {
        return Err(invalid(format!(
            "pack {pack}: a withholding entry needs a kind and a source"
        )));
    }
    let rate_bp = match &raw.percent {
        None => None,
        Some(percent) => Some(percent_to_bp(percent).ok_or_else(|| {
            invalid(format!(
                "pack {pack}: withholding rate {percent:?} is not a percentage"
            ))
        })?),
    };
    if raw.verified && rate_bp.is_none() && raw.note.is_empty() {
        return Err(invalid(format!(
            "pack {pack}: an unrated withholding entry needs a note"
        )));
    }
    Ok(Withholding {
        kind: raw.kind.clone(),
        rate_bp,
        source: raw.source.clone(),
        verified: raw.verified,
        note: raw.note.clone(),
    })
}

/// A percentage text with at most two decimals (`"10"`, `"9.5"`) as basis
/// points (`1000`, `950`); `None` for anything else.
#[must_use]
pub fn percent_to_bp(percent: &str) -> Option<i64> {
    let (whole, frac) = percent.split_once('.').unwrap_or((percent, ""));
    if whole.is_empty() && frac.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !frac.bytes().all(|b| b.is_ascii_digit())
        || frac.len() > 2
    {
        return None;
    }
    let whole: i64 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let frac_bp: i64 = format!("{frac:0<2}").parse().ok()?;
    whole.checked_mul(100)?.checked_add(frac_bp)
}

/// Every bundled pack, parsed and validated.
///
/// # Errors
/// The first pack file that fails [`CountryPack::parse`].
pub fn bundled_packs() -> Result<Vec<CountryPack>, TaxError> {
    BUNDLED
        .iter()
        .map(|(folder, text)| {
            let pack = CountryPack::parse(text)?;
            if pack.id != *folder {
                return Err(invalid(format!(
                    "pack in folder {folder} declares id {}",
                    pack.id
                )));
            }
            Ok(pack)
        })
        .collect()
}

/// The pack a company's country uses. Returns `None` when no bundled pack
/// covers the country; the server then uses [`generic_pack`].
///
/// # Errors
/// A bundled pack file fails to parse (a build defect).
pub fn pack_for_country(country: &str) -> Result<Option<CountryPack>, TaxError> {
    Ok(bundled_packs()?
        .into_iter()
        .find(|pack| pack.covers_country(country)))
}

/// The generic pack: company-defined rates, for countries without a pack.
///
/// # Errors
/// A bundled pack file fails to parse (a build defect).
pub fn generic_pack() -> Result<CountryPack, TaxError> {
    bundled_packs()?
        .into_iter()
        .find(|pack| pack.id == "generic")
        .ok_or_else(|| invalid("the generic pack is missing"))
}

/// A pack by its folder id (`sg`, `de`).
///
/// # Errors
/// [`TaxError::Invalid`] when no bundled pack has the id.
pub fn pack_by_id(id: &str) -> Result<CountryPack, TaxError> {
    bundled_packs()?
        .into_iter()
        .find(|pack| pack.id == id)
        .ok_or_else(|| invalid(format!("no bundled pack {id:?}")))
}

#[cfg(test)]
mod withholding_tests {
    use super::*;

    #[test]
    fn a_percentage_becomes_basis_points_exactly() {
        assert_eq!(percent_to_bp("10"), Some(1000));
        assert_eq!(percent_to_bp("20"), Some(2000));
        assert_eq!(percent_to_bp("9.5"), Some(950));
        assert_eq!(percent_to_bp("0.25"), Some(25));
        assert_eq!(
            percent_to_bp("1.234"),
            None,
            "more than two decimals is refused"
        );
        assert_eq!(percent_to_bp("ten"), None);
        assert_eq!(percent_to_bp(""), None);
    }

    #[test]
    fn the_bundled_packs_carry_their_sourced_withholding_and_notes() {
        let sg = pack_by_id("sg").expect("sg");
        let royalty = sg
            .withholding
            .iter()
            .find(|w| w.kind == "royalty")
            .expect("sg royalty");
        assert_eq!(royalty.rate_bp, Some(1000));
        assert!(royalty.verified && royalty.source.contains("iras.gov.sg"));

        let id = pack_by_id("id").expect("id");
        let royalty = id
            .withholding
            .iter()
            .find(|w| w.kind == "royalty")
            .expect("id royalty");
        assert_eq!(royalty.rate_bp, Some(2000));
        assert!(royalty.verified && royalty.source.contains("pajak.go.id"));

        let nl = pack_by_id("nl").expect("nl");
        assert!(nl.withholding.is_empty());
        let note = nl
            .notes
            .iter()
            .find(|n| n.topic == "innovatiebox")
            .expect("note");
        assert!(note.verified && note.text.contains("9%"));
    }

    #[test]
    fn an_unrated_withholding_entry_needs_its_reason() {
        let raw = RawWithholding {
            kind: "royalty".into(),
            percent: None,
            source: "example".into(),
            verified: true,
            note: String::new(),
        };
        assert!(parse_withholding("xx", &raw).is_err());
        let with_note = RawWithholding {
            note: "rate not verified".into(),
            ..raw
        };
        let parsed = parse_withholding("xx", &with_note).expect("parses with a note");
        assert_eq!(parsed.rate_bp, None);
    }
}
