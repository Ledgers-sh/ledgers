use ledgers_money::{Amount, Currency, Rate};
use ledgers_tax::{
    ReturnClass, VatBasis, VatRecord, VatReturnError, prepare_ec_sales_list,
    prepare_generic_return, prepare_oss, prepare_ustva,
};
use time::Date;

fn eur() -> Currency {
    Currency::from_code("EUR").expect("EUR")
}

#[allow(clippy::too_many_lines)]
fn fixture() -> Vec<VatRecord> {
    let e = eur();
    vec![
        VatRecord {
            tax_point: Date::from_calendar_date(2026, time::Month::January, 10).unwrap(),
            paid_on: Some(Date::from_calendar_date(2026, time::Month::January, 15).unwrap()),
            class: ReturnClass::DomesticStandard,
            net: Amount::from_major(1000, e).unwrap(),
            vat: Amount::new(19_000, e),
        },
        VatRecord {
            tax_point: Date::from_calendar_date(2026, time::Month::January, 12).unwrap(),
            paid_on: Some(Date::from_calendar_date(2026, time::Month::January, 20).unwrap()),
            class: ReturnClass::DomesticStandard,
            net: Amount::new(12_345, e),
            vat: Amount::new(2_346, e),
        },
        VatRecord {
            tax_point: Date::from_calendar_date(2026, time::Month::February, 5).unwrap(),
            paid_on: Some(Date::from_calendar_date(2026, time::Month::February, 10).unwrap()),
            class: ReturnClass::DomesticReduced,
            net: Amount::from_major(500, e).unwrap(),
            vat: Amount::new(3_500, e),
        },
        VatRecord {
            tax_point: Date::from_calendar_date(2026, time::Month::February, 8).unwrap(),
            paid_on: None,
            class: ReturnClass::DomesticStandard,
            net: Amount::new(-10_000, e),
            vat: Amount::new(-1_900, e),
        },
        VatRecord {
            tax_point: Date::from_calendar_date(2026, time::Month::January, 15).unwrap(),
            paid_on: None,
            class: ReturnClass::IntraEuSupply {
                vat_id: "ATU12345678".to_string(),
            },
            net: Amount::from_major(2000, e).unwrap(),
            vat: Amount::new(0, e),
        },
        VatRecord {
            tax_point: Date::from_calendar_date(2026, time::Month::February, 20).unwrap(),
            paid_on: None,
            class: ReturnClass::IntraEuSupply {
                vat_id: "atu 12345678".to_string(),
            },
            net: Amount::new(15_000, e),
            vat: Amount::new(0, e),
        },
        VatRecord {
            tax_point: Date::from_calendar_date(2026, time::Month::March, 1).unwrap(),
            paid_on: None,
            class: ReturnClass::IntraEuSupply {
                vat_id: "FR12345678901".to_string(),
            },
            net: Amount::new(7_550, e),
            vat: Amount::new(0, e),
        },
        VatRecord {
            tax_point: Date::from_calendar_date(2026, time::Month::January, 20).unwrap(),
            paid_on: None,
            class: ReturnClass::IntraEuService {
                vat_id: "BE0123456789".to_string(),
            },
            net: Amount::from_major(500, e).unwrap(),
            vat: Amount::new(0, e),
        },
        VatRecord {
            tax_point: Date::from_calendar_date(2026, time::Month::February, 15).unwrap(),
            paid_on: None,
            class: ReturnClass::ExemptWithDeduction,
            net: Amount::from_major(300, e).unwrap(),
            vat: Amount::new(0, e),
        },
        VatRecord {
            tax_point: Date::from_calendar_date(2026, time::Month::February, 10).unwrap(),
            paid_on: None,
            class: ReturnClass::ExemptWithoutDeduction,
            net: Amount::from_major(250, e).unwrap(),
            vat: Amount::new(0, e),
        },
        VatRecord {
            tax_point: Date::from_calendar_date(2026, time::Month::January, 25).unwrap(),
            paid_on: None,
            class: ReturnClass::IntraEuAcquisitionStandard,
            net: Amount::new(10_000, e),
            vat: Amount::new(1_900, e),
        },
        VatRecord {
            tax_point: Date::from_calendar_date(2026, time::Month::March, 15).unwrap(),
            paid_on: None,
            class: ReturnClass::IntraEuAcquisitionReduced,
            net: Amount::new(14_000, e),
            vat: Amount::new(980, e),
        },
        VatRecord {
            tax_point: Date::from_calendar_date(2026, time::Month::February, 5).unwrap(),
            paid_on: None,
            class: ReturnClass::ReverseChargeEuService,
            net: Amount::new(40_000, e),
            vat: Amount::new(7_600, e),
        },
        VatRecord {
            tax_point: Date::from_calendar_date(2026, time::Month::January, 30).unwrap(),
            paid_on: None,
            class: ReturnClass::ReverseChargeOther,
            net: Amount::new(30_000, e),
            vat: Amount::new(5_700, e),
        },
        VatRecord {
            tax_point: Date::from_calendar_date(2026, time::Month::March, 5).unwrap(),
            paid_on: None,
            class: ReturnClass::InputVat,
            net: Amount::from_major(200, e).unwrap(),
            vat: Amount::new(3_800, e),
        },
        VatRecord {
            tax_point: Date::from_calendar_date(2026, time::Month::February, 28).unwrap(),
            paid_on: None,
            class: ReturnClass::ImportVat,
            net: Amount::new(5_000, e),
            vat: Amount::new(950, e),
        },
        VatRecord {
            tax_point: Date::from_calendar_date(2026, time::Month::January, 5).unwrap(),
            paid_on: None,
            class: ReturnClass::OssDistanceSale {
                country: "IT".to_string(),
                rate: Rate::parse("0.22").unwrap(),
            },
            net: Amount::new(10_000, e),
            vat: Amount::new(2_200, e),
        },
        VatRecord {
            tax_point: Date::from_calendar_date(2026, time::Month::March, 10).unwrap(),
            paid_on: None,
            class: ReturnClass::OssDistanceSale {
                country: "IT".to_string(),
                rate: Rate::parse("0.22").unwrap(),
            },
            net: Amount::new(5_000, e),
            vat: Amount::new(1_100, e),
        },
        VatRecord {
            tax_point: Date::from_calendar_date(2026, time::Month::February, 1).unwrap(),
            paid_on: None,
            class: ReturnClass::OssDistanceSale {
                country: "ES".to_string(),
                rate: Rate::parse("0.21").unwrap(),
            },
            net: Amount::new(8_000, e),
            vat: Amount::new(1_680, e),
        },
        VatRecord {
            tax_point: Date::from_calendar_date(2026, time::Month::March, 25).unwrap(),
            paid_on: Some(Date::from_calendar_date(2026, time::Month::April, 5).unwrap()),
            class: ReturnClass::DomesticStandard,
            net: Amount::new(20_000, e),
            vat: Amount::new(3_800, e),
        },
        // tax_point Q1, paid Q2 (excluded cash, included accrual)
        VatRecord {
            tax_point: Date::from_calendar_date(2026, time::Month::March, 15).unwrap(),
            paid_on: Some(Date::from_calendar_date(2026, time::Month::April, 10).unwrap()),
            class: ReturnClass::DomesticReduced,
            net: Amount::from_major(300, e).unwrap(),
            vat: Amount::new(2_100, e),
        },
        // tax_point Q4 2025, paid Q1 2026 (included cash, excluded accrual)
        VatRecord {
            tax_point: Date::from_calendar_date(2025, time::Month::December, 20).unwrap(),
            paid_on: Some(Date::from_calendar_date(2026, time::Month::January, 5).unwrap()),
            class: ReturnClass::DomesticReduced,
            net: Amount::from_major(100, e).unwrap(),
            vat: Amount::new(700, e),
        },
        // tax_point Q1, unpaid (included accrual, excluded cash - tests Istversteuerung fix)
        VatRecord {
            tax_point: Date::from_calendar_date(2026, time::Month::March, 20).unwrap(),
            paid_on: None,
            class: ReturnClass::DomesticStandard,
            net: Amount::new(5_000, e),
            vat: Amount::new(950, e),
        },
    ]
}

#[test]
fn ustva_accrual_equals_fixture() -> Result<(), VatReturnError> {
    let from = Date::from_calendar_date(2026, time::Month::January, 1).unwrap();
    let to = Date::from_calendar_date(2026, time::Month::March, 31).unwrap();
    let result = prepare_ustva(&fixture(), from, to, VatBasis::Accrual)?;

    // Expected boxes in EUR (test divides amount.minor() by 100)
    let expected_boxes = [
        ("81", 1273),
        ("81-tax", 241),
        ("86", 800),
        ("86-tax", 56),
        ("41", 2225),
        ("21", 500),
        ("43", 300),
        ("48", 250),
        ("89", 100),
        ("89-tax", 19),
        ("93", 140),
        ("93-tax", 9),
        ("46", 400),
        ("47", 76),
        ("84", 300),
        ("85", 57),
        ("66", 38),
        ("61", 28),
        ("62", 9),
        ("67", 133),
        ("83", 250),
    ];

    for (kz, minor) in expected_boxes {
        let found = result
            .boxes
            .iter()
            .find(|b| b.kz == kz)
            .map(|b| b.amount.minor() / 100);
        assert_eq!(found, Some(i128::from(minor)), "Kz {kz} mismatch");
    }
    Ok(())
}

#[test]
fn ustva_cash_basis_moves_boxes() -> Result<(), VatReturnError> {
    let from = Date::from_calendar_date(2026, time::Month::January, 1).unwrap();
    let to = Date::from_calendar_date(2026, time::Month::March, 31).unwrap();
    let accrual = prepare_ustva(&fixture(), from, to, VatBasis::Accrual)?;
    let cash = prepare_ustva(&fixture(), from, to, VatBasis::Cash)?;
    let accrual_83 = accrual
        .boxes
        .iter()
        .find(|b| b.kz == "83")
        .map(|b| b.amount.minor());
    let cash_83 = cash
        .boxes
        .iter()
        .find(|b| b.kz == "83")
        .map(|b| b.amount.minor());
    assert_ne!(accrual_83, cash_83, "Cash and accrual Kz 83 should differ");
    Ok(())
}

#[test]
fn ec_sales_list_matches_fixture_and_kz41_kz21() -> Result<(), VatReturnError> {
    let from = Date::from_calendar_date(2026, time::Month::January, 1).unwrap();
    let to = Date::from_calendar_date(2026, time::Month::March, 31).unwrap();
    let rows = prepare_ec_sales_list(&fixture(), from, to)?;
    let expected = [
        ("ATU12345678", "AT", "L", 2150),
        ("BE0123456789", "BE", "S", 500),
        ("FR12345678901", "FR", "L", 75),
    ];
    assert_eq!(rows.len(), expected.len(), "EC Sales List row count");
    for (i, (exp_id, exp_country, exp_kind, exp_minor)) in expected.iter().enumerate() {
        assert_eq!(rows[i].vat_id, *exp_id);
        assert_eq!(rows[i].country, *exp_country);
        assert_eq!(rows[i].kind, *exp_kind);
        assert_eq!(rows[i].amount.minor(), exp_minor * 100);
    }

    let ustva = prepare_ustva(&fixture(), from, to, VatBasis::Accrual)?;
    let goods_sum: i128 = rows
        .iter()
        .filter(|r| r.kind == "L")
        .map(|r| r.amount.minor())
        .sum();
    let services_sum: i128 = rows
        .iter()
        .filter(|r| r.kind == "S")
        .map(|r| r.amount.minor())
        .sum();
    let kz41 = ustva
        .boxes
        .iter()
        .find(|b| b.kz == "41")
        .map_or(0, |b| b.amount.minor());
    let kz21 = ustva
        .boxes
        .iter()
        .find(|b| b.kz == "21")
        .map_or(0, |b| b.amount.minor());
    assert_eq!(goods_sum, kz41, "EC Sales goods sum = Kz 41");
    assert_eq!(services_sum, kz21, "EC Sales services sum = Kz 21");
    Ok(())
}

#[test]
fn oss_matches_fixture() -> Result<(), VatReturnError> {
    let from = Date::from_calendar_date(2026, time::Month::January, 1).unwrap();
    let to = Date::from_calendar_date(2026, time::Month::March, 31).unwrap();
    let rows = prepare_oss(&fixture(), from, to)?;
    assert_eq!(rows.len(), 2, "OSS row count");
    assert_eq!(
        (
            rows[0].country.as_str(),
            rows[0].net.minor(),
            rows[0].vat.minor()
        ),
        ("ES", 8000, 1680)
    );
    assert_eq!(
        (
            rows[1].country.as_str(),
            rows[1].net.minor(),
            rows[1].vat.minor()
        ),
        ("IT", 15000, 3300)
    );
    Ok(())
}

#[test]
fn generic_return_net_payable() -> Result<(), VatReturnError> {
    let from = Date::from_calendar_date(2026, time::Month::January, 1).unwrap();
    let to = Date::from_calendar_date(2026, time::Month::March, 31).unwrap();
    let r = prepare_generic_return(&fixture(), from, to, VatBasis::Accrual, "EUR")?;
    let computed = r.output_vat.checked_sub(r.input_vat)?;
    assert_eq!(
        r.net_payable, computed,
        "net_payable must equal output − input"
    );
    Ok(())
}

#[test]
fn errors_unsupported_year_period_span_from_gt_to_non_eur_de_vat_oss_country() {
    let from = Date::from_calendar_date(2025, time::Month::January, 1).unwrap();
    let to = Date::from_calendar_date(2025, time::Month::December, 31).unwrap();
    assert!(matches!(
        prepare_ustva(&fixture(), from, to, VatBasis::Accrual),
        Err(VatReturnError::UnsupportedTaxYear(2025))
    ));

    let from = Date::from_calendar_date(2025, time::Month::December, 1).unwrap();
    let to = Date::from_calendar_date(2026, time::Month::January, 31).unwrap();
    assert!(matches!(
        prepare_ustva(&fixture(), from, to, VatBasis::Accrual),
        Err(VatReturnError::UnsupportedTaxYear(2025))
    ));

    let from = Date::from_calendar_date(2026, time::Month::March, 31).unwrap();
    let to = Date::from_calendar_date(2026, time::Month::January, 1).unwrap();
    assert!(matches!(
        prepare_ustva(&fixture(), from, to, VatBasis::Accrual),
        Err(VatReturnError::InvalidPeriod)
    ));

    let e = eur();
    let gbp = Currency::from_code("GBP").unwrap();
    let recs = vec![VatRecord {
        tax_point: Date::from_calendar_date(2026, time::Month::January, 1).unwrap(),
        paid_on: None,
        class: ReturnClass::DomesticStandard,
        net: Amount::from_major(100, gbp).unwrap(),
        vat: Amount::new(19_00, gbp),
    }];
    assert!(matches!(
        prepare_ustva(
            &recs,
            Date::from_calendar_date(2026, time::Month::January, 1).unwrap(),
            Date::from_calendar_date(2026, time::Month::March, 31).unwrap(),
            VatBasis::Accrual
        ),
        Err(VatReturnError::Currency(_))
    ));

    let recs = vec![VatRecord {
        tax_point: Date::from_calendar_date(2026, time::Month::January, 1).unwrap(),
        paid_on: None,
        class: ReturnClass::IntraEuSupply {
            vat_id: "DE123456789".to_string(),
        },
        net: Amount::from_major(100, e).unwrap(),
        vat: Amount::new(0, e),
    }];
    assert!(matches!(
        prepare_ec_sales_list(
            &recs,
            Date::from_calendar_date(2026, time::Month::January, 1).unwrap(),
            Date::from_calendar_date(2026, time::Month::March, 31).unwrap()
        ),
        Err(VatReturnError::InvalidVatId(_))
    ));

    let recs = vec![VatRecord {
        tax_point: Date::from_calendar_date(2026, time::Month::January, 1).unwrap(),
        paid_on: None,
        class: ReturnClass::OssDistanceSale {
            country: "DE".to_string(),
            rate: Rate::parse("0.19").unwrap(),
        },
        net: Amount::from_major(100, e).unwrap(),
        vat: Amount::new(19_00, e),
    }];
    assert!(matches!(
        prepare_oss(
            &recs,
            Date::from_calendar_date(2026, time::Month::January, 1).unwrap(),
            Date::from_calendar_date(2026, time::Month::March, 31).unwrap()
        ),
        Err(VatReturnError::InvalidVatId(_))
    ));
}
