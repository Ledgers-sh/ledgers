//! Integration test for `ledgers demo init`.

use ledgers_cli::demo;

#[tokio::test]
async fn demo_init_is_idempotent_with_a_temp_database() {
    let temp_dir = std::env::temp_dir();
    let db_file = temp_dir.join(format!("ledgers-demo-test-{}.db", std::process::id()));
    let db_path = db_file.to_string_lossy().to_string();
    let blob_dir = temp_dir
        .join(".ledgers-blobs")
        .to_string_lossy()
        .to_string();
    let company_id = "test-demo-company";
    let year = 2026;

    // Clean up any existing database
    let _ = std::fs::remove_file(&db_path);

    // First run should post entries
    let first = demo::init(&db_path, company_id, &blob_dir, true, year)
        .await
        .expect("first run succeeds");
    // Every fixture entry: journal entries plus the link-only offset.
    assert_eq!(first.posted, 178, "first run posts all entries");
    assert_eq!(first.reversed, 1, "first run posts reversals");
    assert_eq!(first.dimensions_created, 4, "first run creates dimensions");

    // Second run should post nothing new
    let second = demo::init(&db_path, company_id, &blob_dir, true, year)
        .await
        .expect("second run succeeds");
    assert_eq!(
        second.posted, 0,
        "second run posts nothing (all are skipped)"
    );
    assert_eq!(second.reversed, 0, "second run posts no new reversals");
    assert_eq!(
        second.dimensions_created, 0,
        "second run creates no new dimensions"
    );
    assert_eq!(
        second.skipped, 179,
        "second run skips all 179 entries (178 posts and offsets + 1 reversal)"
    );

    // Clean up
    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_dir_all(&blob_dir);
}
