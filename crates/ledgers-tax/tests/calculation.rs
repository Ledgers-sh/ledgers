//! Tax calculation tests with table-driven fixtures.
//!
//! Fixture table: each row is (inputs) → (expected net, tax, gross), rationale.
//!
//! 1. `net_standard_19`: 100.00 EUR at 19%, per-line `HalfEven`
//!    Expected: net=100.00, tax=19.00, gross=119.00
//!    Rationale: Basic net-based tax, standard rate, no rounding needed
//!
//! 2. `net_reduced_7`: 100.00 EUR at 7%, per-line `HalfEven`
//!    Expected: net=100.00, tax=7.00, gross=107.00
//!    Rationale: Net-based with reduced rate, plain case
//!
//! 3. `half_even_tie_7`: 1.50 EUR at 7%, per-line `HalfEven` vs `HalfUp`
//!    Product: 1.50 × 0.07 = 0.105; in scaled: 150 × 70M = 10.5B,
//!    Quotient=10 (even), remainder=500M (exactly half)
//!    `HalfEven`: quotient even, no bump → 0.10
//!    `HalfUp`: remainder=half, bump → 0.11
//!    Rationale: Tests tie-breaking between `HalfEven` and `HalfUp`
//!
//! 4. `per_document_vs_per_line`: Three lines of 0.03 EUR at 7%
//!    Per-line: 0.03 × 7% = 0.0021 → rounds to 0.00 each; total 0.00
//!    Per-document: (0.03+0.03+0.03) × 7% = 0.09 × 7% = 0.0063 → 0.01
//!    Rationale: Demonstrates per-line vs per-document rounding difference
//!
//! 5. `gross_10_at_19`: 10.00 EUR gross at 19%, per-line `HalfEven`
//!    tax = round(10.00 × 0.19 / 1.19) = round(1.596...) = 1.60 EUR
//!    net = 10.00 - 1.60 = 8.40 EUR
//!    Verify: 8.40 + 1.60 = 10.00 ✓
//!    Rationale: Gross-based, net+tax==gross exactly
//!
//! 6. `gross_0_01_at_19`: 0.01 EUR gross at 19%
//!    tax = round(0.01 × 0.19 / 1.19) = round(0.001597...) = 0.00 EUR
//!    net = 0.01 EUR
//!    Rationale: Awkward amount with no tax
//!
//! 7. `gross_1_19_at_19`: 1.19 EUR gross at 19%
//!    tax = round(1.19 × 0.19 / 1.19) = round(0.19) = 0.19 EUR
//!    net = 1.00 EUR
//!    Rationale: Exact computation, nice round numbers
//!
//! 8. `reverse_charge_balanced`: 100.00 EUR net, `OutputVat` and `InputVat` both 19.00
//!    Rationale: `ReverseCharge` entries have matching output and input VAT
//!
//! 9. `small_business_exemption`: 100.00 EUR at 19%, `small_business=true`
//!    Expected: `output_vat=0.00`, input note present
//!    Rationale: Small business exemption suppresses `OutputVat`
//!
//! 10. `temporary_rate_2020`: 100.00 EUR at temporary 16% rate (2020-07-01)
//!     Expected: tax=16.00 (not 19.00)
//!     Rationale: Date-based rate lookup for temporary reduction

use ledgers_money::{Amount, Currency, Rate, Rounding};
use ledgers_tax::{
    Basis, CalculateParams, DatedRate, Jurisdiction, RateCategory, RateTable, TaxCode, TaxKind,
    TaxLineInput, calculate,
};
use time::Date;

fn eur() -> Currency {
    Currency::from_code("EUR").expect("EUR is valid")
}

fn code(kind: TaxKind, cat: RateCategory, out: Option<&str>, inp: Option<&str>) -> TaxCode {
    TaxCode::new(
        "test".to_string(),
        kind,
        cat,
        out.map(str::to_string),
        inp.map(str::to_string),
        Rate::ZERO,
    )
    .unwrap()
}

// ============================================================================
// Core acceptance tests (existing, must keep passing)
// ============================================================================

#[test]
fn test_net_standard_19() {
    let rates = ledgers_tax::pack_by_id("de")
        .unwrap()
        .rate_table(&[])
        .unwrap();
    let test_code = code(
        TaxKind::OutputVat,
        RateCategory::Standard,
        Some("1200"),
        None,
    );

    let line = TaxLineInput {
        amount: Amount::from_major(100, eur()).unwrap(),
        tax_code: "test".to_string(),
    };

    let params = CalculateParams {
        lines: &[line],
        date: Date::from_calendar_date(2021, time::Month::January, 1).unwrap(),
        table: &rates,
        jurisdiction: &Jurisdiction::new("DE"),
        code_lookup: &|_| Ok(test_code.clone()),
        basis: Basis::Net,
        scope: ledgers_tax::RoundingScope::PerLine,
        rounding: Rounding::HalfEven,
        small_business: false,
    };

    let calc = calculate(&params).unwrap();
    assert_eq!(calc.total_net.to_canonical_string(), "100.00");
    assert_eq!(calc.total_output_vat.to_canonical_string(), "19.00");
    assert_eq!(calc.total_gross.to_canonical_string(), "119.00");
}

#[test]
fn test_net_reduced_7() {
    let rates = ledgers_tax::pack_by_id("de")
        .unwrap()
        .rate_table(&[])
        .unwrap();
    let test_code = code(
        TaxKind::OutputVat,
        RateCategory::Reduced,
        Some("1200"),
        None,
    );

    let line = TaxLineInput {
        amount: Amount::from_major(100, eur()).unwrap(),
        tax_code: "test".to_string(),
    };

    let params = CalculateParams {
        lines: &[line],
        date: Date::from_calendar_date(2021, time::Month::January, 1).unwrap(),
        table: &rates,
        jurisdiction: &Jurisdiction::new("DE"),
        code_lookup: &|_| Ok(test_code.clone()),
        basis: Basis::Net,
        scope: ledgers_tax::RoundingScope::PerLine,
        rounding: Rounding::HalfEven,
        small_business: false,
    };

    let calc = calculate(&params).unwrap();
    assert_eq!(calc.total_net.to_canonical_string(), "100.00");
    assert_eq!(calc.total_output_vat.to_canonical_string(), "7.00");
    assert_eq!(calc.total_gross.to_canonical_string(), "107.00");
}

#[test]
fn test_temporary_rate() {
    let rates = ledgers_tax::pack_by_id("de")
        .unwrap()
        .rate_table(&[])
        .unwrap();
    let test_code = code(
        TaxKind::OutputVat,
        RateCategory::Standard,
        Some("1200"),
        None,
    );

    let line = TaxLineInput {
        amount: Amount::from_major(100, eur()).unwrap(),
        tax_code: "test".to_string(),
    };

    let params = CalculateParams {
        lines: &[line],
        date: Date::from_calendar_date(2020, time::Month::July, 1).unwrap(),
        table: &rates,
        jurisdiction: &Jurisdiction::new("DE"),
        code_lookup: &|_| Ok(test_code.clone()),
        basis: Basis::Net,
        scope: ledgers_tax::RoundingScope::PerLine,
        rounding: Rounding::HalfEven,
        small_business: false,
    };

    let calc = calculate(&params).unwrap();
    assert_eq!(calc.total_output_vat.to_canonical_string(), "16.00");
}

#[test]
fn test_gross_based() {
    let rates = ledgers_tax::pack_by_id("de")
        .unwrap()
        .rate_table(&[])
        .unwrap();
    let test_code = code(
        TaxKind::OutputVat,
        RateCategory::Standard,
        Some("1200"),
        None,
    );

    let line = TaxLineInput {
        amount: Amount::from_major(119, eur()).unwrap(),
        tax_code: "test".to_string(),
    };

    let params = CalculateParams {
        lines: &[line],
        date: Date::from_calendar_date(2021, time::Month::January, 1).unwrap(),
        table: &rates,
        jurisdiction: &Jurisdiction::new("DE"),
        code_lookup: &|_| Ok(test_code.clone()),
        basis: Basis::Gross,
        scope: ledgers_tax::RoundingScope::PerLine,
        rounding: Rounding::HalfEven,
        small_business: false,
    };

    let calc = calculate(&params).unwrap();
    assert_eq!(calc.total_net.to_canonical_string(), "100.00");
    assert_eq!(calc.total_output_vat.to_canonical_string(), "19.00");
    assert_eq!(calc.total_gross.to_canonical_string(), "119.00");
}

#[test]
fn test_reverse_charge() {
    let rates = ledgers_tax::pack_by_id("de")
        .unwrap()
        .rate_table(&[])
        .unwrap();
    let test_code = code(
        TaxKind::ReverseCharge,
        RateCategory::Standard,
        Some("1200"),
        Some("1410"),
    );

    let line = TaxLineInput {
        amount: Amount::from_major(100, eur()).unwrap(),
        tax_code: "test".to_string(),
    };

    let params = CalculateParams {
        lines: &[line],
        date: Date::from_calendar_date(2021, time::Month::January, 1).unwrap(),
        table: &rates,
        jurisdiction: &Jurisdiction::new("DE"),
        code_lookup: &|_| Ok(test_code.clone()),
        basis: Basis::Net,
        scope: ledgers_tax::RoundingScope::PerLine,
        rounding: Rounding::HalfEven,
        small_business: false,
    };

    let calc = calculate(&params).unwrap();
    assert_eq!(calc.total_output_vat.to_canonical_string(), "19.00");
    assert_eq!(calc.total_input_vat.to_canonical_string(), "19.00");
}

#[test]
fn test_small_business() {
    let rates = ledgers_tax::pack_by_id("de")
        .unwrap()
        .rate_table(&[])
        .unwrap();
    let test_code = code(
        TaxKind::OutputVat,
        RateCategory::Standard,
        Some("1200"),
        None,
    );

    let line = TaxLineInput {
        amount: Amount::from_major(100, eur()).unwrap(),
        tax_code: "test".to_string(),
    };

    let params = CalculateParams {
        lines: &[line],
        date: Date::from_calendar_date(2021, time::Month::January, 1).unwrap(),
        table: &rates,
        jurisdiction: &Jurisdiction::new("DE"),
        code_lookup: &|_| Ok(test_code.clone()),
        basis: Basis::Net,
        scope: ledgers_tax::RoundingScope::PerLine,
        rounding: Rounding::HalfEven,
        small_business: true,
    };

    let calc = calculate(&params).unwrap();
    assert_eq!(calc.total_output_vat.to_canonical_string(), "0.00");
    assert!(calc.small_business_note.is_some());
}

#[test]
fn test_rate_gaps() {
    let dates = vec![
        DatedRate {
            jurisdiction: Jurisdiction::new("DE"),
            category: RateCategory::Standard,
            rate: "0.19".parse().unwrap(),
            valid_from: Date::from_calendar_date(2020, time::Month::January, 1).unwrap(),
            valid_to: Some(Date::from_calendar_date(2020, time::Month::June, 30).unwrap()),
            source: "Test".to_string(),
        },
        DatedRate {
            jurisdiction: Jurisdiction::new("DE"),
            category: RateCategory::Standard,
            rate: "0.19".parse().unwrap(),
            valid_from: Date::from_calendar_date(2021, time::Month::January, 1).unwrap(),
            valid_to: None,
            source: "Test".to_string(),
        },
    ];
    assert!(RateTable::new(dates).is_err());
}

#[test]
fn test_rate_overlaps() {
    let dates = vec![
        DatedRate {
            jurisdiction: Jurisdiction::new("DE"),
            category: RateCategory::Standard,
            rate: "0.19".parse().unwrap(),
            valid_from: Date::from_calendar_date(2020, time::Month::January, 1).unwrap(),
            valid_to: Some(Date::from_calendar_date(2020, time::Month::December, 31).unwrap()),
            source: "Test".to_string(),
        },
        DatedRate {
            jurisdiction: Jurisdiction::new("DE"),
            category: RateCategory::Standard,
            rate: "0.16".parse().unwrap(),
            valid_from: Date::from_calendar_date(2020, time::Month::July, 1).unwrap(),
            valid_to: Some(Date::from_calendar_date(2020, time::Month::December, 31).unwrap()),
            source: "Test".to_string(),
        },
    ];
    assert!(RateTable::new(dates).is_err());
}

// ============================================================================
// Rounding and edge case tests
// ============================================================================

#[test]
fn test_half_even_vs_half_up_tie_at_7_percent() {
    // 1.50 EUR at 7% creates an exact tie (remainder = 0.5 × divisor)
    // Product: 150 × 70_000_000_000 = 10_500_000_000_000
    // Quotient: 10 (even), remainder: 500_000_000_000 (exactly half)
    // HalfEven: don't bump (quotient even) → 0.10 EUR
    // HalfUp: bump → 0.11 EUR
    let rates = ledgers_tax::pack_by_id("de")
        .unwrap()
        .rate_table(&[])
        .unwrap();
    let test_code = code(
        TaxKind::OutputVat,
        RateCategory::Reduced,
        Some("1200"),
        None,
    );

    let line = TaxLineInput {
        amount: Amount::new(150, eur()), // 1.50 EUR in cents
        tax_code: "test".to_string(),
    };

    // Test HalfEven
    let params_even = CalculateParams {
        lines: std::slice::from_ref(&line),
        date: Date::from_calendar_date(2021, time::Month::January, 1).unwrap(),
        table: &rates,
        jurisdiction: &Jurisdiction::new("DE"),
        code_lookup: &|_| Ok(test_code.clone()),
        basis: Basis::Net,
        scope: ledgers_tax::RoundingScope::PerLine,
        rounding: Rounding::HalfEven,
        small_business: false,
    };
    let calc_even = calculate(&params_even).unwrap();
    assert_eq!(calc_even.total_output_vat.to_canonical_string(), "0.10");

    // Test HalfUp
    let params_up = CalculateParams {
        lines: &[line],
        date: Date::from_calendar_date(2021, time::Month::January, 1).unwrap(),
        table: &rates,
        jurisdiction: &Jurisdiction::new("DE"),
        code_lookup: &|_| Ok(test_code.clone()),
        basis: Basis::Net,
        scope: ledgers_tax::RoundingScope::PerLine,
        rounding: Rounding::HalfUp,
        small_business: false,
    };
    let calc_up = calculate(&params_up).unwrap();
    assert_eq!(calc_up.total_output_vat.to_canonical_string(), "0.11");
}

#[test]
fn test_per_document_rounding_differs_from_per_line() {
    // Three lines of 0.03 EUR at 7%, per-line vs per-document
    // Per-line: 0.03 × 7% = 0.0021 → rounds to 0.00 each; total 0.00 tax
    // Per-document: 0.09 × 7% = 0.0063 → rounds to 0.01 tax (allocated to lines)
    let rates = ledgers_tax::pack_by_id("de")
        .unwrap()
        .rate_table(&[])
        .unwrap();
    let test_code = code(
        TaxKind::OutputVat,
        RateCategory::Reduced,
        Some("1200"),
        None,
    );

    let lines = vec![
        TaxLineInput {
            amount: Amount::new(3, eur()), // 0.03 EUR in cents
            tax_code: "test".to_string(),
        },
        TaxLineInput {
            amount: Amount::new(3, eur()),
            tax_code: "test".to_string(),
        },
        TaxLineInput {
            amount: Amount::new(3, eur()),
            tax_code: "test".to_string(),
        },
    ];

    // Per-line calculation
    let params_per_line = CalculateParams {
        lines: &lines,
        date: Date::from_calendar_date(2021, time::Month::January, 1).unwrap(),
        table: &rates,
        jurisdiction: &Jurisdiction::new("DE"),
        code_lookup: &|_| Ok(test_code.clone()),
        basis: Basis::Net,
        scope: ledgers_tax::RoundingScope::PerLine,
        rounding: Rounding::HalfEven,
        small_business: false,
    };
    let calc_per_line = calculate(&params_per_line).unwrap();
    assert_eq!(calc_per_line.total_output_vat.to_canonical_string(), "0.00");

    // Per-document calculation
    let params_per_doc = CalculateParams {
        lines: &lines,
        date: Date::from_calendar_date(2021, time::Month::January, 1).unwrap(),
        table: &rates,
        jurisdiction: &Jurisdiction::new("DE"),
        code_lookup: &|_| Ok(test_code.clone()),
        basis: Basis::Net,
        scope: ledgers_tax::RoundingScope::PerDocument,
        rounding: Rounding::HalfEven,
        small_business: false,
    };
    let calc_per_doc = calculate(&params_per_doc).unwrap();
    assert_eq!(calc_per_doc.total_output_vat.to_canonical_string(), "0.01");
    // Verify lines sum to the document total
    let line_taxes_sum: i128 = calc_per_doc
        .lines
        .iter()
        .map(|(_, l)| l.output_vat.minor())
        .sum();
    assert_eq!(line_taxes_sum, calc_per_doc.total_output_vat.minor());
}

#[test]
fn test_gross_based_net_plus_tax_equals_gross() {
    // Verify that for gross-based calculations, net + tax == gross exactly
    let rates = ledgers_tax::pack_by_id("de")
        .unwrap()
        .rate_table(&[])
        .unwrap();
    let test_code = code(
        TaxKind::OutputVat,
        RateCategory::Standard,
        Some("1200"),
        None,
    );

    // Test case: 10.00 EUR gross at 19%
    // Expected: tax = round(10.00 × 0.19 / 1.19) = 1.60, net = 8.40
    let line_10 = TaxLineInput {
        amount: Amount::from_major(10, eur()).unwrap(),
        tax_code: "test".to_string(),
    };

    let params = CalculateParams {
        lines: &[line_10],
        date: Date::from_calendar_date(2021, time::Month::January, 1).unwrap(),
        table: &rates,
        jurisdiction: &Jurisdiction::new("DE"),
        code_lookup: &|_| Ok(test_code.clone()),
        basis: Basis::Gross,
        scope: ledgers_tax::RoundingScope::PerLine,
        rounding: Rounding::HalfEven,
        small_business: false,
    };

    let calc = calculate(&params).unwrap();
    assert_eq!(calc.total_gross.to_canonical_string(), "10.00");
    // Verify: net + tax == gross
    let net_plus_tax = calc.total_net.checked_add(calc.total_output_vat).unwrap();
    assert_eq!(net_plus_tax, calc.total_gross);
}

#[test]
fn test_gross_based_small_amounts() {
    // Test gross-based with amounts that don't divide evenly
    let rates = ledgers_tax::pack_by_id("de")
        .unwrap()
        .rate_table(&[])
        .unwrap();
    let test_code = code(
        TaxKind::OutputVat,
        RateCategory::Standard,
        Some("1200"),
        None,
    );

    // 0.01 EUR gross at 19%: tax rounds to 0.00, net = 0.01
    let line_001 = TaxLineInput {
        amount: Amount::new(1, eur()), // 0.01 EUR in cents
        tax_code: "test".to_string(),
    };

    let params = CalculateParams {
        lines: &[line_001],
        date: Date::from_calendar_date(2021, time::Month::January, 1).unwrap(),
        table: &rates,
        jurisdiction: &Jurisdiction::new("DE"),
        code_lookup: &|_| Ok(test_code.clone()),
        basis: Basis::Gross,
        scope: ledgers_tax::RoundingScope::PerLine,
        rounding: Rounding::HalfEven,
        small_business: false,
    };

    let calc = calculate(&params).unwrap();
    assert_eq!(calc.total_gross.to_canonical_string(), "0.01");
    assert_eq!(calc.total_output_vat.to_canonical_string(), "0.00");
    assert_eq!(calc.total_net.to_canonical_string(), "0.01");
    // Verify: net + tax == gross
    let net_plus_tax = calc.total_net.checked_add(calc.total_output_vat).unwrap();
    assert_eq!(net_plus_tax, calc.total_gross);
}

#[test]
fn test_gross_based_1_19() {
    // 1.19 EUR gross at 19% should give tax = 0.19, net = 1.00 exactly
    let rates = ledgers_tax::pack_by_id("de")
        .unwrap()
        .rate_table(&[])
        .unwrap();
    let test_code = code(
        TaxKind::OutputVat,
        RateCategory::Standard,
        Some("1200"),
        None,
    );

    let line = TaxLineInput {
        amount: Amount::new(119, eur()), // 1.19 EUR in cents
        tax_code: "test".to_string(),
    };

    let params = CalculateParams {
        lines: &[line],
        date: Date::from_calendar_date(2021, time::Month::January, 1).unwrap(),
        table: &rates,
        jurisdiction: &Jurisdiction::new("DE"),
        code_lookup: &|_| Ok(test_code.clone()),
        basis: Basis::Gross,
        scope: ledgers_tax::RoundingScope::PerLine,
        rounding: Rounding::HalfEven,
        small_business: false,
    };

    let calc = calculate(&params).unwrap();
    assert_eq!(calc.total_gross.to_canonical_string(), "1.19");
    assert_eq!(calc.total_output_vat.to_canonical_string(), "0.19");
    assert_eq!(calc.total_net.to_canonical_string(), "1.00");
}
