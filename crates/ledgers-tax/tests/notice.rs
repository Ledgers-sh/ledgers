//! Verify that the root NOTICE file mentions the bundled German tax rates.

#[test]
fn notice_mentions_german_tax_rates() {
    let notice = include_str!("../../../NOTICE");

    // Must mention the rates file
    assert!(
        notice.contains("crates/ledgers-tax/src/rate.rs"),
        "NOTICE must mention crates/ledgers-tax/src/rate.rs (German VAT rates)"
    );

    // Must mention the bundled rates
    assert!(
        notice.contains("19%") && notice.contains("7%"),
        "NOTICE must mention the standard (19%) and reduced (7%) German VAT rates"
    );

    // Must mention the statute
    assert!(notice.contains("§12 UStG"), "NOTICE must cite §12 UStG");
}
