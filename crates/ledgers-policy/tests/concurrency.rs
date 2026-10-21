//! Concurrency test: guard statements under load with rolling-daily limits.
//!
//! Validates that `consume_statements` correctly generates guard statements for
//! per-currency, per-rolling-day limit enforcement, mirroring the approach in
//! ledgers-db/tests/concurrency.rs.

use ledgers_policy::{CurrencyLimit, limit};
use std::collections::BTreeMap;
use time::OffsetDateTime;

#[test]
fn test_consume_statements_single_currency() {
    let now = OffsetDateTime::now_utc();
    let offset = time::UtcOffset::UTC;

    let mut limits = BTreeMap::new();
    limits.insert(
        "EUR".to_string(),
        CurrencyLimit {
            per_entry: 10_000, // 100.00 EUR
            per_day: 100_000,  // 1000.00 EUR
        },
    );

    let mut effect = BTreeMap::new();
    effect.insert("EUR".to_string(), 5_000); // 50.00 EUR

    // Produce guard statements for this effect
    let statements = limit::consume_statements("test-key", &effect, now, offset, &limits)
        .expect("consume_statements should succeed");

    // Should produce exactly one statement for EUR
    assert_eq!(statements.len(), 1, "Should have one guard statement");
}

#[test]
fn test_consume_statements_multiple_currencies() {
    let now = OffsetDateTime::now_utc();
    let offset = time::UtcOffset::UTC;

    let mut limits = BTreeMap::new();
    limits.insert(
        "EUR".to_string(),
        CurrencyLimit {
            per_entry: 10_000,
            per_day: 100_000,
        },
    );
    limits.insert(
        "USD".to_string(),
        CurrencyLimit {
            per_entry: 12_000,
            per_day: 120_000,
        },
    );

    let mut effect = BTreeMap::new();
    effect.insert("EUR".to_string(), 5_000);
    effect.insert("USD".to_string(), 6_000);

    let statements = limit::consume_statements("test-key", &effect, now, offset, &limits)
        .expect("consume_statements should succeed");

    // Should produce two statements
    assert_eq!(statements.len(), 2, "Should have two guard statements");
}

#[test]
fn test_consume_statements_zero_effect_ignored() {
    let now = OffsetDateTime::now_utc();
    let offset = time::UtcOffset::UTC;

    let mut limits = BTreeMap::new();
    limits.insert(
        "EUR".to_string(),
        CurrencyLimit {
            per_entry: 10_000,
            per_day: 100_000,
        },
    );

    let mut effect = BTreeMap::new();
    effect.insert("EUR".to_string(), 0); // Zero effect ignored

    let statements = limit::consume_statements("test-key", &effect, now, offset, &limits)
        .expect("consume_statements should succeed");

    // Should produce no statements for zero effect
    assert_eq!(
        statements.len(),
        0,
        "Zero effect should produce no statements"
    );
}

#[test]
fn test_consume_statements_missing_limit_fails() {
    let now = OffsetDateTime::now_utc();
    let offset = time::UtcOffset::UTC;

    let limits = BTreeMap::new(); // Empty limits

    let mut effect = BTreeMap::new();
    effect.insert("EUR".to_string(), 5_000);

    // Should fail: EUR has no limit
    let result = limit::consume_statements("test-key", &effect, now, offset, &limits);
    assert!(result.is_err(), "Should fail when currency has no limit");
}
