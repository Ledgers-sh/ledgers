//! Verify that the root NOTICE file mentions all bundled data sources in
//! this crate, particularly the ISO 4217 currency table.

#[test]
fn notice_mentions_iso4217_currency_table() {
    let notice = include_str!("../../../NOTICE");

    // Must mention the currency table file
    assert!(
        notice.contains("crates/ledgers-money/src/currency.rs"),
        "NOTICE must mention crates/ledgers-money/src/currency.rs (ISO 4217 table)"
    );

    // Must mention ISO 4217
    assert!(notice.contains("ISO 4217"), "NOTICE must mention ISO 4217");

    // Must mention the data sources (datasets/currency-codes)
    assert!(
        notice.contains("datasets/currency-codes"),
        "NOTICE must mention datasets/currency-codes as a source"
    );

    // Must mention the licence (ODC-PDDL)
    assert!(
        notice.contains("ODC-PDDL"),
        "NOTICE must mention the ODC-PDDL licence for datasets/currency-codes"
    );
}
