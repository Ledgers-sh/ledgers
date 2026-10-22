//! Country packs: every bundled pack loads and validates, the registration
//! validators accept and reject known vectors, the pack a country resolves to,
//! the generic return per regime type, and the core stays free of a "DE"
//! literal.

use ledgers_money::{Amount, Currency};
use ledgers_tax::country::{self, Regime};
use ledgers_tax::rate::{Jurisdiction, RateCategory};
use ledgers_tax::settings::VatBasis;
use ledgers_tax::vat_return::{ReturnClass, VatRecord, prepare_generic_return};
use ledgers_tax::{Validator, bundled_packs, generic_pack, pack_by_id, pack_for_country};
use time::{Date, Month};

fn date(y: i32, m: Month, d: u8) -> Date {
    Date::from_calendar_date(y, m, d).unwrap()
}

#[test]
fn every_bundled_pack_loads_and_matches_its_folder() {
    let packs = bundled_packs().expect("every bundled pack parses and validates");
    let ids: Vec<&str> = packs.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["de", "sg", "id", "nl", "gb", "au", "us", "eu", "generic"]
    );
}

#[test]
fn rates_are_sane_and_dated_in_order() {
    for pack in bundled_packs().unwrap() {
        for rate in &pack.rates {
            let percent = rate.rate.to_string();
            let value: f64 = percent.parse().unwrap();
            assert!(
                (0.0..=1.0).contains(&value),
                "{}: rate {percent} out of range",
                pack.id
            );
            if let Some(to) = rate.valid_to {
                assert!(
                    rate.valid_from <= to,
                    "{}: valid_to before valid_from",
                    pack.id
                );
            }
            assert!(
                !rate.source.trim().is_empty(),
                "{}: rate without source",
                pack.id
            );
        }
        // Gaps and overlaps are checked when the table is built; a pack whose
        // statutory rows were out of order would fail here.
        pack.rate_table(&[])
            .unwrap_or_else(|e| panic!("{}: {e}", pack.id));
    }
}

#[test]
fn reduced_rates_sit_below_the_standard_rate_on_each_day() {
    let de = pack_by_id("de").unwrap();
    let table = de.rate_table(&[]).unwrap();
    let day = date(2026, Month::October, 23);
    let standard = table
        .rate_on(&Jurisdiction::new("DE"), RateCategory::Standard, day)
        .unwrap()
        .rate
        .to_string();
    let reduced = table
        .rate_on(&Jurisdiction::new("DE"), RateCategory::Reduced, day)
        .unwrap()
        .rate
        .to_string();
    assert_eq!(standard, "0.19");
    assert_eq!(reduced, "0.07");
}

#[test]
fn singapore_pack_has_the_stable_gst_codes() {
    let sg = pack_by_id("sg").unwrap();
    assert_eq!(sg.regime, Regime::Gst);
    assert_eq!(sg.currency, Some(Currency::from_code("SGD").unwrap()));
    let accounts = [
        ("vat-output".to_owned(), "2020".to_owned()),
        ("vat-input".to_owned(), "1050".to_owned()),
    ]
    .into_iter()
    .collect();
    let codes: Vec<String> = sg
        .tax_codes(&accounts)
        .unwrap()
        .into_iter()
        .map(|c| c.code)
        .collect();
    assert_eq!(
        codes,
        vec!["SG-GST-SR-9", "SG-GST-ZR", "SG-GST-ES", "SG-GST-TX"]
    );
}

#[test]
fn a_pack_with_a_tag_its_chart_lacks_is_refused() {
    let sg = pack_by_id("sg").unwrap();
    let empty = std::collections::BTreeMap::new();
    assert!(sg.tax_codes(&empty).is_err());
}

#[test]
fn registration_validators_accept_and_reject_known_vectors() {
    let cases: &[(Validator, &str, bool)] = &[
        // German USt-IdNr: the ISO 7064 check digit of the published example.
        (Validator::Iso7064Nine, "DE136695976", true),
        (Validator::Iso7064Nine, "DE136695977", false),
        (Validator::Iso7064Nine, "DE12345678", false),
        // UK VAT: HMRC example GB 999 9999 73 (mod-97 check).
        (Validator::GbVat, "GB999999973", true),
        (Validator::GbVat, "GB999999974", false),
        (Validator::GbVat, "GB99999997", false),
        // Australian ABN: 51 824 753 556 passes the ATO check (sum divisible by 89).
        (Validator::AuAbn, "51824753556", true),
        (Validator::AuAbn, "51824753557", false),
        (Validator::AuAbn, "0182475355", false),
        // Singapore UEN shapes.
        (Validator::SgUen, "53012345K", true),
        (Validator::SgUen, "201912345K", true),
        (Validator::SgUen, "T08LL1234A", true),
        (Validator::SgUen, "5301234K", false),
        (Validator::SgUen, "53012345", false),
        // Indonesian NPWP: 15 or 16 digits.
        (Validator::IdNpwp, "012345678901234", true),
        (Validator::IdNpwp, "0123456789012345", true),
        (Validator::IdNpwp, "01234567890123", false),
        (Validator::IdNpwp, "01234567890123A", false),
        // Dutch BTW-id.
        (Validator::NlBtw, "NL123456782B12", true),
        (Validator::NlBtw, "NL12345678B12", false),
        (Validator::NlBtw, "NL123456782C12", false),
        // US EIN, separators removed first.
        (Validator::UsEin, &Validator::normalize("12-3456789"), true),
        (Validator::UsEin, &Validator::normalize("12-345678"), false),
        // EU: listed states follow their pattern; unlisted states take the shape.
        (Validator::EuVat, "FRAB123456789", true),
        (Validator::EuVat, "FR123456789", false),
        (Validator::EuVat, "NLX", false),
        (Validator::EuVat, "PT123456789", true),
        (Validator::EuVat, "XX123456789", true),
    ];
    for (validator, number, expected) in cases {
        let got = validator.check(number).is_ok();
        assert_eq!(
            got, *expected,
            "{validator:?} {number}: expected {expected}"
        );
    }
}

#[test]
fn validators_normalize_separators_and_case() {
    assert_eq!(Validator::normalize(" de 136.695-976 "), "DE136695976");
    assert_eq!(Validator::normalize("nl-123456782 b12"), "NL123456782B12");
}

#[test]
fn a_country_resolves_to_its_pack() {
    let pack = |c: &str| pack_for_country(c).unwrap().map(|p| p.id);
    assert_eq!(pack("SG").as_deref(), Some("sg"));
    assert_eq!(pack("DE").as_deref(), Some("de"));
    assert_eq!(pack("NL").as_deref(), Some("nl"));
    assert_eq!(pack("FR").as_deref(), Some("eu"));
    assert_eq!(pack("US").as_deref(), Some("us"));
    assert_eq!(pack("JP"), None);
    assert_eq!(generic_pack().unwrap().id, "generic");
}

// --- Generic return, one golden per regime type -------------------------------

fn rec(class: ReturnClass, net: i128, vat: i128, currency: Currency) -> VatRecord {
    VatRecord {
        tax_point: date(2026, Month::March, 10),
        paid_on: Some(date(2026, Month::March, 12)),
        class,
        net: Amount::new(net, currency),
        vat: Amount::new(vat, currency),
    }
}

fn period() -> (Date, Date) {
    (date(2026, Month::January, 1), date(2026, Month::March, 31))
}

#[test]
fn generic_return_for_a_vat_regime() {
    // A UK-style VAT return in GBP: 1000.00 at 20%, 100.00 at 5%, 500.00 zero, 300.00 input.
    let gbp = Currency::from_code("GBP").unwrap();
    let records = vec![
        rec(ReturnClass::DomesticStandard, 100_000, 20_000, gbp),
        rec(ReturnClass::DomesticReduced, 10_000, 500, gbp),
        rec(ReturnClass::ZeroRated, 50_000, 0, gbp),
        rec(ReturnClass::InputVat, 0, 30_000, gbp),
    ];
    let (from, to) = period();
    let r = prepare_generic_return(&records, from, to, VatBasis::Accrual, "GBP").unwrap();
    assert_eq!(r.output_vat.minor(), 20_500);
    assert_eq!(r.input_vat.minor(), 30_000);
    assert_eq!(r.net_payable.minor(), -9_500);
    assert_eq!(r.standard_net.minor(), 100_000);
    assert_eq!(r.reduced_net.minor(), 10_000);
    assert_eq!(r.zero_rated_net.minor(), 50_000);
    assert_eq!(r.zero_rated_and_exempt_sales.minor(), 50_000);
    assert_eq!(r.taxable_sales.minor(), 110_000);
}

#[test]
fn generic_return_for_a_gst_regime_includes_reverse_charge_on_both_sides() {
    // Singapore-style GST in SGD: 1000.00 at 9%, 200.00 exempt, reverse-charge 50.00 at 9%.
    let sgd = Currency::from_code("SGD").unwrap();
    let records = vec![
        rec(ReturnClass::DomesticStandard, 100_000, 9_000, sgd),
        rec(ReturnClass::ExemptWithoutDeduction, 20_000, 0, sgd),
        rec(ReturnClass::ReverseChargeOther, 50_000, 4_500, sgd),
    ];
    let (from, to) = period();
    let r = prepare_generic_return(&records, from, to, VatBasis::Accrual, "SGD").unwrap();
    assert_eq!(r.output_vat.minor(), 13_500);
    assert_eq!(r.input_vat.minor(), 4_500);
    assert_eq!(r.net_payable.minor(), 9_000);
    assert_eq!(r.zero_rated_and_exempt_sales.minor(), 20_000);
    assert_eq!(r.currency.code(), "SGD");
}

#[test]
fn generic_return_for_a_sales_tax_regime_is_a_liability_on_the_sale() {
    // US-style: sales tax collected is output; tax paid on purchases is input at
    // the company's rate. 2000.00 taxed at 8.25%, 500.00 purchases with 41.25 tax.
    let usd = Currency::from_code("USD").unwrap();
    let records = vec![
        rec(ReturnClass::DomesticStandard, 200_000, 16_500, usd),
        rec(ReturnClass::InputVat, 0, 4_125, usd),
    ];
    let (from, to) = period();
    let r = prepare_generic_return(&records, from, to, VatBasis::Accrual, "USD").unwrap();
    assert_eq!(r.output_vat.minor(), 16_500);
    assert_eq!(r.net_payable.minor(), 12_375);
}

#[test]
fn generic_return_for_no_tax_regime_is_all_zero() {
    let aud = Currency::from_code("AUD").unwrap();
    let (from, to) = period();
    let r = prepare_generic_return(&[], from, to, VatBasis::Accrual, "AUD").unwrap();
    assert_eq!(r.output_vat.minor(), 0);
    assert_eq!(r.input_vat.minor(), 0);
    assert_eq!(r.net_payable.minor(), 0);
    assert_eq!(r.currency, aud);
}

#[test]
fn generic_return_refuses_an_unknown_currency_and_mixed_currencies() {
    let (from, to) = period();
    assert!(prepare_generic_return(&[], from, to, VatBasis::Accrual, "XYZ").is_err());
    let eur = Currency::from_code("EUR").unwrap();
    let usd = Currency::from_code("USD").unwrap();
    let mixed = vec![
        rec(ReturnClass::DomesticStandard, 100, 19, eur),
        rec(ReturnClass::InputVat, 0, 5, usd),
    ];
    assert!(prepare_generic_return(&mixed, from, to, VatBasis::Accrual, "EUR").is_err());
}

#[test]
fn a_return_box_layout_names_figures_the_generic_return_has() {
    let sg = pack_by_id("sg").unwrap();
    let f5 = sg
        .returns
        .iter()
        .find_map(|r| match r {
            country::ReturnLayout::Boxes { id, boxes, .. } if id == "sg-gst-f5" => Some(boxes),
            _ => None,
        })
        .expect("the SG F5 layout");
    let numbers: Vec<&str> = f5.iter().map(|b| b.number.as_str()).collect();
    assert_eq!(numbers, vec!["1", "2", "3", "6", "7", "8"]);
}

#[test]
fn the_core_has_no_german_literal() {
    // Core code paths must not name a country. The UStVA builder is German by
    // design and sits in its own module, which this test does not scan.
    let files = [
        ("calc.rs", include_str!("../src/calc.rs")),
        ("code.rs", include_str!("../src/code.rs")),
        ("country.rs", include_str!("../src/country.rs")),
        ("rate.rs", include_str!("../src/rate.rs")),
        ("settings.rs", include_str!("../src/settings.rs")),
        ("validate.rs", include_str!("../src/validate.rs")),
        ("posting.rs", include_str!("../src/posting.rs")),
        (
            "server settings.rs",
            include_str!("../../ledgers-server/src/dispatch/settings.rs"),
        ),
        (
            "server settings_rules.rs",
            include_str!("../../ledgers-server/src/dispatch/settings_rules.rs"),
        ),
    ];
    for (name, text) in files {
        // Test modules may name countries; production code may not.
        let production = text.split("#[cfg(test)]").next().unwrap();
        assert!(
            !production.contains("\"DE\""),
            "{name} names the DE country literally"
        );
        assert!(!production.contains("Germany"), "{name} names Germany");
    }
}
