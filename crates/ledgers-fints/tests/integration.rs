use ledgers_fints::{DefaultPinSource, FintsFeed, FintsFeedConfig};
use std::sync::Arc;
use tracing::Level;
use tracing_subscriber::fmt::Subscriber;

// Test helper: validate that PIN is not in logs or errors
#[test]
fn test_pin_not_leaked_in_config_debug() {
    let _guard = setup_tracing();

    let cfg = FintsFeedConfig::new_with_blz("10010010", "user", "DE89370400440532013000", None)
        .with_product_id("test");

    let debug_str = format!("{cfg:?}");
    assert!(!debug_str.contains("pin"));
    assert!(!debug_str.contains("PIN"));
}

#[test]
fn test_pin_not_leaked_in_feed_debug() {
    let _guard = setup_tracing();

    let cfg = FintsFeedConfig::new_with_blz("10010010", "user", "DE89370400440532013000", None)
        .with_product_id("test");
    let source = DefaultPinSource::new(|_| Some("HW-pin-7319".to_string()));
    let feed = FintsFeed::new(cfg, Arc::new(source)).unwrap();

    let debug_str = format!("{feed:?}");
    assert!(!debug_str.contains("HW-pin-7319"));
    assert!(!debug_str.contains("7319"));
}

#[test]
fn test_duplicate_transactions_distinct_ids() {
    let mut seen = std::collections::HashSet::new();

    // Same coffee transaction on same day - first occurrence
    let id1 = ledgers_fints::feed::generate_external_id(
        "DE89370400440532013000",
        "2026-01-15",
        Some("2026-01-15"),
        "5.50",
        "EUR",
        Some("COFFEE"),
        "Coffee",
        &seen,
    );
    seen.insert(id1.clone());

    // Identical coffee on same day - second occurrence
    let id2 = ledgers_fints::feed::generate_external_id(
        "DE89370400440532013000",
        "2026-01-15",
        Some("2026-01-15"),
        "5.50",
        "EUR",
        Some("COFFEE"),
        "Coffee",
        &seen,
    );

    // Must be different due to occurrence index
    assert_ne!(id1, id2);
    assert!(id1.starts_with("fints:"));
    assert!(id2.starts_with("fints:"));
}

#[test]
fn test_resync_stable_ids() {
    // Simulate first sync of 2026-01-01 to 2026-01-31
    let mut first_sync_ids = std::collections::HashSet::new();
    let mut first_seen = std::collections::HashSet::new();

    let tx_a = ledgers_fints::feed::generate_external_id(
        "DE89370400440532013000",
        "2026-01-15",
        Some("2026-01-15"),
        "100.00",
        "EUR",
        Some("VENDOR-A"),
        "Invoice A",
        &first_seen,
    );
    first_sync_ids.insert(tx_a.clone());
    first_seen.insert(tx_a.clone());

    let tx_b = ledgers_fints::feed::generate_external_id(
        "DE89370400440532013000",
        "2026-01-20",
        Some("2026-01-20"),
        "200.00",
        "EUR",
        Some("VENDOR-B"),
        "Invoice B",
        &first_seen,
    );
    first_sync_ids.insert(tx_b.clone());
    first_seen.insert(tx_b.clone());

    // Simulate second sync of 2026-01-20 to 2026-02-10 (overlapping)
    let mut second_sync_ids = std::collections::HashSet::new();
    let mut second_seen = std::collections::HashSet::new();

    // Same transactions as before should have identical IDs
    let tx_a_resync = ledgers_fints::feed::generate_external_id(
        "DE89370400440532013000",
        "2026-01-15",
        Some("2026-01-15"),
        "100.00",
        "EUR",
        Some("VENDOR-A"),
        "Invoice A",
        &second_seen,
    );
    second_sync_ids.insert(tx_a_resync.clone());
    second_seen.insert(tx_a_resync.clone());

    let tx_b_resync = ledgers_fints::feed::generate_external_id(
        "DE89370400440532013000",
        "2026-01-20",
        Some("2026-01-20"),
        "200.00",
        "EUR",
        Some("VENDOR-B"),
        "Invoice B",
        &second_seen,
    );
    second_sync_ids.insert(tx_b_resync.clone());
    second_seen.insert(tx_b_resync.clone());

    // New transaction in second sync
    let tx_c = ledgers_fints::feed::generate_external_id(
        "DE89370400440532013000",
        "2026-02-05",
        Some("2026-02-05"),
        "50.00",
        "EUR",
        Some("VENDOR-C"),
        "Invoice C",
        &second_seen,
    );
    second_sync_ids.insert(tx_c.clone());

    // Verify IDs are stable across syncs
    assert_eq!(tx_a, tx_a_resync);
    assert_eq!(tx_b, tx_b_resync);

    // Merge: {A, B, C} (no duplicates)
    let mut merged = std::collections::HashSet::new();
    merged.extend(first_sync_ids);
    merged.extend(second_sync_ids);
    assert_eq!(merged.len(), 3); // A, B, C (A and B from first sync, deduplicated in merge)
}

#[test]
fn test_pin_source_multiple_calls() {
    let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let call_count_clone = call_count.clone();

    let source = DefaultPinSource::new(move |_| {
        call_count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Some("test-pin".to_string())
    });

    // Wrap in Arc for the feed
    let source_arc: Arc<dyn ledgers_fints::PinSource> = Arc::new(source);

    // Creating feeds doesn't call the source; only pulling does
    let cfg = FintsFeedConfig::new_with_blz("10010010", "user", "DE89370400440532013000", None)
        .with_product_id("test");

    let _feed1 = FintsFeed::new(cfg.clone(), source_arc.clone()).unwrap();
    let _feed2 = FintsFeed::new(cfg, source_arc).unwrap();

    // Feeds created but not called; count should still be 0
    assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 0);
}

fn setup_tracing() -> Option<()> {
    let _ = Subscriber::builder()
        .with_max_level(Level::TRACE)
        .with_writer(std::io::sink)
        .finish();
    None
}
