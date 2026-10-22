use crate::*;
use ledgers_money::{Amount, Currency};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn test_secure_pin_redaction() {
    let pin = SecurePin::new("secret-7319".to_string());
    assert_eq!(format!("{pin:?}"), "****");
    assert_eq!(format!("{pin}"), "****");
    assert!(!format!("{pin:?}").contains("secret"));
}

#[test]
fn test_config_validation() {
    let cfg = FintsFeedConfig::new_with_blz("10010010", "", "DE89370400440532013000", None);
    assert!(cfg.validate().is_err());

    let cfg = FintsFeedConfig::new_with_blz("10010010", "user", "", None);
    assert!(cfg.validate().is_err());

    let cfg = FintsFeedConfig::new_with_blz("10010010", "user", "DE89370400440532013000", None);
    assert!(cfg.validate().is_ok());
}

#[test]
fn test_product_id_from_env() {
    let cfg = FintsFeedConfig::new_with_blz("10010010", "user", "DE89370400440532013000", None);
    // Will fail if env var not set (expected in test)
    let result = cfg.product_id_source.resolve();
    if std::env::var("LEDGERS_FINTS_PRODUCT_ID").is_err() {
        assert!(result.is_err());
    }
}

#[test]
fn test_product_id_explicit() {
    let cfg = FintsFeedConfig::new_with_blz("10010010", "user", "DE89370400440532013000", None)
        .with_product_id("test-id");
    assert_eq!(cfg.product_id_source.resolve().unwrap(), "test-id");
}

#[test]
fn test_external_id_stable() {
    let mut seen = std::collections::HashSet::new();

    let id1 = crate::feed::generate_external_id(
        "DE89370400440532013000",
        "2026-01-15",
        Some("2026-01-15"),
        "100.50",
        "EUR",
        Some("DE89370400440532013000"),
        "test",
        &seen,
    );
    seen.insert(id1.clone());

    let id2 = crate::feed::generate_external_id(
        "DE89370400440532013000",
        "2026-01-15",
        Some("2026-01-15"),
        "100.50",
        "EUR",
        Some("DE89370400440532013000"),
        "test",
        &seen,
    );

    // Different occurrence indices due to seen_ids
    assert_ne!(id1, id2);
    assert!(id1.starts_with("fints:"));
    assert!(id2.starts_with("fints:"));
}

#[tokio::test]
async fn test_default_pin_source() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_clone = call_count.clone();

    let source = DefaultPinSource::new(move |_| {
        call_count_clone.fetch_add(1, Ordering::SeqCst);
        Some("pin".to_string())
    });

    let pin = source.get_pin("account").await.unwrap();
    assert_eq!(pin.expose(), "pin");
    assert_eq!(call_count.load(Ordering::SeqCst), 1);

    // Second call
    let _pin2 = source.get_pin("account2").await.unwrap();
    assert_eq!(call_count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_default_pin_source_cancel() {
    let source = DefaultPinSource::new(|_| None);
    let result = source.get_pin("account").await;
    assert!(matches!(result, Err(FintsFeedError::Cancelled)));
}

#[cfg(feature = "keychain")]
#[tokio::test]
async fn test_keychain_feature() {
    let ks = KeychainPinSource::new("test-service");
    // Just verify it constructs without error
    assert_eq!(format!("{:?}", ks).contains("KeychainPinSource"), true);
}

#[test]
fn test_amount_conversion() {
    let amt = Amount::parse_canonical("1234.56", Currency::from_code("EUR").unwrap()).unwrap();
    assert_eq!(amt.minor(), 123_456);

    let amt_neg = Amount::parse_canonical("-500.00", Currency::from_code("EUR").unwrap()).unwrap();
    assert_eq!(amt_neg.minor(), -50_000);
}

#[test]
fn test_feed_debug() {
    let cfg = FintsFeedConfig::new_with_blz("10010010", "user", "DE89370400440532013000", None)
        .with_product_id("test");
    let source = DefaultPinSource::new(|_| Some("pin".to_string()));
    let feed = FintsFeed::new(cfg, Arc::new(source)).unwrap();
    let debug_str = format!("{feed:?}");
    assert!(debug_str.contains("FintsFeed"));
    assert!(!debug_str.contains("pin")); // PIN must not appear
}
