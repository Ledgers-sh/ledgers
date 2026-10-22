//! Dutch BTW: the statutory rates by date, the stable tax codes, the
//! registration validators, the BTW-aangifte golden, the ICP draft and the KOR
//! rules. Sources are cited in `templates/countries/nl/pack.toml` and in the
//! module docs of `ledgers_tax::nl`.

use ledgers_money::{Amount, Currency};
use ledgers_tax::nl::{IcpSoort, KorStatus, kor_status, prepare_btw_aangifte_nl, prepare_icp_nl};
use ledgers_tax::rate::{Jurisdiction, RateCategory};
use ledgers_tax::settings::VatBasis;
use ledgers_tax::vat_return::{ReturnClass, VatRecord};
use ledgers_tax::{Validator, pack_by_id};
use time::{Date, Month};

fn date(y: i32, m: Month, d: u8) -> Date {
    Date::from_calendar_date(y, m, d).unwrap()
}

fn eur(minor: i128) -> Amount {
    Amount::new(minor, Currency::from_code("EUR").unwrap())
}

fn rec(day: Date, class: ReturnClass, net: i128, vat: i128) -> VatRecord {
    VatRecord {
        tax_point: day,
        paid_on: None,
        class,
        net: eur(net),
        vat: eur(vat),
    }
}

#[test]
fn nl_rates_are_dated_from_the_statute_history() {
    let table = pack_by_id("nl").unwrap().rate_table(&[]).unwrap();
    let nl = Jurisdiction::new("NL");
    let rate = |category, day| {
        table
            .rate_on(&nl, category, day)
            .map(|dated| dated.rate.to_string())
    };
    // 21% from 2012-10-01: the first day and a current day.
    assert_eq!(
        rate(RateCategory::Standard, date(2012, Month::October, 1)).unwrap(),
        "0.21"
    );
    assert_eq!(
        rate(RateCategory::Standard, date(2026, Month::October, 23)).unwrap(),
        "0.21"
    );
    // 6% from 2012-10-01 to 2018-12-31; 9% from 2019-01-01.
    assert_eq!(
        rate(RateCategory::Reduced, date(2018, Month::December, 31)).unwrap(),
        "0.06"
    );
    assert_eq!(
        rate(RateCategory::Reduced, date(2019, Month::January, 1)).unwrap(),
        "0.09"
    );
    assert_eq!(
        rate(RateCategory::Reduced, date(2026, Month::October, 23)).unwrap(),
        "0.09"
    );
    // Before 2012-10-01 the pack holds no sourced rate: the lookup fails.
    assert!(rate(RateCategory::Standard, date(2012, Month::September, 30)).is_err());
}

#[test]
fn nl_pack_names_the_stable_tax_codes() {
    let pack = pack_by_id("nl").unwrap();
    for code in [
        "NL-BTW-HOOG-21",
        "NL-BTW-LAAG-9",
        "NL-BTW-NUL",
        "NL-BTW-VRIJ",
        "NL-BTW-VERLEGD",
        "NL-BTW-ICP",
        "NL-BTW-IMPORT",
        "NL-BTW-VOORB",
    ] {
        assert!(
            pack.tax_codes.iter().any(|c| c.code == code),
            "pack nl lacks {code}"
        );
    }
}

#[test]
fn dutch_validators_accept_and_reject_shapes() {
    // BTW-id: shape only (check digits are a TODO in the pack).
    assert!(Validator::NlBtw.check("NL123456782B12").is_ok());
    assert!(Validator::NlBtw.check("NL12345678B12").is_err());
    assert!(Validator::NlBtw.check("NL123456782C12").is_err());
    // KvK-nummer: eight digits.
    let kvk = Validator::from_code("nl-kvk").unwrap();
    assert!(kvk.check("12345678").is_ok());
    assert!(kvk.check("1234567").is_err());
    assert!(kvk.check("1234567A").is_err());
    // EU VAT number for an intra-community customer.
    assert!(Validator::EuVat.check("DE123456789").is_ok());
}

#[test]
fn kor_limit_and_prior_year_rule() {
    let limit = eur(2_000_000);
    let below = eur(1_999_999);
    let over = eur(2_000_001);
    let none = eur(0);
    assert_eq!(kor_status(false, over, over).unwrap(), KorStatus::NotChosen);
    assert_eq!(kor_status(true, below, none).unwrap(), KorStatus::Within);
    assert_eq!(kor_status(true, limit, none).unwrap(), KorStatus::AtLimit);
    assert_eq!(kor_status(true, over, none).unwrap(), KorStatus::Exceeded);
    assert_eq!(
        kor_status(true, none, over).unwrap(),
        KorStatus::PriorYearExceeded
    );
}

/// The Dutch BV fixture for Q1 2026. Every figure is hand-computed:
///
/// | record                       | net        | VAT        | box |
/// |------------------------------|------------|------------|-----|
/// | domestic 21%                 | 1000.00    | 210.00     | 1a  |
/// | domestic 21%                 | 500.10     | 105.02     | 1a  |
/// | domestic 9%                  | 200.00     | 18.00      | 1b  |
/// | zero-rated export            | 300.00     | 0          | 1e  |
/// | exempt                       | 100.00     | 0          | 1e  |
/// | reverse charge (domestic)    | 400.00     | 84.00      | 2a  |
/// | intra-EU goods to DE         | 900.00     | 0          | 3b  |
/// | EU acquisition               | 250.00     | 52.50      | 4b  |
/// | import from outside the EU   | 1000.00    | 210.00     | 4a  |
/// | input VAT                    | 800.00     | 168.00     | 5b  |
/// | input VAT                    | 10.00      | 2.01       | 5b  |
fn bv_fixture() -> Vec<VatRecord> {
    let day = date(2026, Month::February, 10);
    vec![
        rec(day, ReturnClass::DomesticStandard, 100_000, 21_000),
        rec(day, ReturnClass::DomesticStandard, 50_010, 10_502),
        rec(day, ReturnClass::DomesticReduced, 20_000, 1_800),
        rec(day, ReturnClass::ZeroRated, 30_000, 0),
        rec(day, ReturnClass::ExemptWithDeduction, 10_000, 0),
        rec(day, ReturnClass::ReverseChargeOther, 40_000, 8_400),
        rec(
            day,
            ReturnClass::IntraEuSupply {
                vat_id: "DE123456789".to_owned(),
            },
            90_000,
            0,
        ),
        rec(day, ReturnClass::IntraEuAcquisitionStandard, 25_000, 5_250),
        rec(day, ReturnClass::ImportVat, 100_000, 21_000),
        rec(day, ReturnClass::InputVat, 80_000, 16_800),
        rec(day, ReturnClass::InputVat, 1_000, 201),
    ]
}

fn q1() -> (Date, Date) {
    (date(2026, Month::January, 1), date(2026, Month::March, 31))
}

#[test]
fn bv_aangifte_golden_matches_the_hand_computed_boxes() {
    let (from, to) = q1();
    let draft = prepare_btw_aangifte_nl(&bv_fixture(), from, to, VatBasis::Accrual, false).unwrap();
    assert!(!draft.kor);
    let box_of = |rubriek: &str| {
        draft
            .rubrieken
            .iter()
            .find(|v| v.rubriek == rubriek)
            .unwrap_or_else(|| panic!("box {rubriek} missing"))
    };
    // VAT in the output boxes is rounded down to whole euros: 315.02 -> 315, 52.50 -> 52.
    assert_eq!(box_of("1a").btw, eur(31_500));
    assert_eq!(box_of("1a").omzet, eur(150_010));
    assert_eq!(box_of("1b").btw, eur(1_800));
    assert_eq!(box_of("1e").omzet, eur(40_000));
    assert_eq!(box_of("2a").btw, eur(8_400));
    assert_eq!(box_of("3b").omzet, eur(90_000));
    assert_eq!(box_of("4a").btw, eur(21_000));
    assert_eq!(box_of("4b").btw, eur(5_200));
    // 5a is the sum of the rounded boxes; 5b is the input VAT rounded up:
    // 170.01 + 210.00 + 84.00 + 52.50 = 516.51 -> 517.
    assert_eq!(draft.verschuldigde_btw, eur(67_900));
    assert_eq!(draft.voorbelasting, eur(51_700));
    assert_eq!(draft.saldo, eur(16_200));
}

#[test]
fn kor_suppresses_the_aangifte() {
    let (from, to) = q1();
    let draft = prepare_btw_aangifte_nl(&bv_fixture(), from, to, VatBasis::Accrual, true).unwrap();
    assert!(draft.kor);
    assert!(
        draft
            .rubrieken
            .iter()
            .all(|v| v.btw.minor() == 0 && v.omzet.minor() == 0)
    );
    assert_eq!(draft.verschuldigde_btw, eur(0));
    assert_eq!(draft.voorbelasting, eur(0));
}

#[test]
fn icp_draft_groups_goods_and_services_per_customer() {
    let (from, to) = q1();
    let day = date(2026, Month::March, 3);
    let mut records = bv_fixture();
    records.push(rec(
        day,
        ReturnClass::IntraEuService {
            vat_id: "DE123456789".to_owned(),
        },
        50_000,
        0,
    ));
    records.push(rec(
        day,
        ReturnClass::IntraEuSupply {
            vat_id: "DE123456789".to_owned(),
        },
        10_000,
        0,
    ));
    let lines = prepare_icp_nl(&records, from, to, VatBasis::Accrual).unwrap();
    assert_eq!(lines.len(), 2);
    let goods = lines
        .iter()
        .find(|l| l.soort == IcpSoort::Goederen)
        .unwrap();
    assert_eq!(goods.btw_id, "DE123456789");
    assert_eq!(goods.omzet, eur(100_000));
    let services = lines
        .iter()
        .find(|l| l.soort == IcpSoort::Diensten)
        .unwrap();
    assert_eq!(services.omzet, eur(50_000));
}

#[test]
fn icp_rejects_a_dutch_customer_and_a_malformed_number() {
    let (from, to) = q1();
    let day = date(2026, Month::March, 3);
    let dutch = vec![rec(
        day,
        ReturnClass::IntraEuSupply {
            vat_id: "NL123456782B12".to_owned(),
        },
        1_000,
        0,
    )];
    assert!(prepare_icp_nl(&dutch, from, to, VatBasis::Accrual).is_err());
    let malformed = vec![rec(
        day,
        ReturnClass::IntraEuService {
            vat_id: "DE1".to_owned(),
        },
        1_000,
        0,
    )];
    assert!(prepare_icp_nl(&malformed, from, to, VatBasis::Accrual).is_err());
}
