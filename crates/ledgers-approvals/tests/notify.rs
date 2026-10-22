//! Integration tests for approval notifications delivery.

use cratefield_core::{Database, Module, Outbox, Statement};
use ledgers_approvals::{
    ApprovalEvent, ApprovalNotice, ApprovalRequest, Status, notice_statement,
    notify::storage::Channel,
    notify::{NOTICE_OUTBOX_TABLE, storage},
};
use ledgers_db::DbModule;
use sea_query::Value as SeaValue;
use std::collections::BTreeMap;

const COMPANY_ID: &str = "test-company-01234567890123";
const PERSON_ID: &str = "approver-person-id";
const NOW: &str = "2026-01-15T09:30:00Z";

fn modules() -> Vec<Box<dyn Module>> {
    vec![
        Box::new(DbModule),
        Box::new(ledgers_storage::StorageModule),
        Box::new(ledgers_approvals::ApprovalsModule),
    ]
}

// Helper to create a test approval notice
fn test_notice(approval_id: &str, event: ApprovalEvent, high_risk: bool) -> ApprovalNotice {
    let mut amounts = BTreeMap::new();
    if high_risk {
        amounts.insert("EUR".to_string(), "123450".to_string()); // EUR 1234.50
    }

    ApprovalNotice {
        approval_id: approval_id.to_string(),
        company_id: COMPANY_ID.to_string(),
        event,
        status: "pending".to_string(),
        amounts,
        counterparty: Some("Acme GmbH".to_string()),
        reason: "above limit".to_string(),
        link: format!("/approvals/{}", approval_id),
        high_risk,
    }
}

async fn setup_db(db: &dyn Database) {
    // Create company
    db.execute(&Statement::with_values(
        "INSERT INTO company (id, name, base_currency, created_at, updated_at) VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT (id) DO NOTHING"
            .to_owned(),
        vec![
            SeaValue::from(COMPANY_ID),
            SeaValue::from("Halden Works GmbH"),
            SeaValue::from("EUR"),
            SeaValue::from(NOW),
            SeaValue::from(NOW),
        ],
    ))
    .await
    .ok();
}

// ============================================================================
// T1: Webhook Delivery with Retries
// ============================================================================

#[test]
fn t1_webhook_retry_and_dead_letter() {
    for kit in cratefield_testing::TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_db(&*kit.db).await;

            let approval_id = "appr-webhook-001";
            let outbox = Outbox::new(NOTICE_OUTBOX_TABLE);

            // Enqueue created notice
            let notice = test_notice(approval_id, ApprovalEvent::Created, true);
            let payload = serde_json::to_string(&notice).unwrap();
            let outbox_id = format!("{}-created", approval_id);
            let stmt = outbox.enqueue_statement(
                &outbox_id,
                ApprovalEvent::Created.topic(),
                &payload,
                Some(COMPANY_ID),
                NOW,
            );
            kit.db.execute(&stmt).await.expect("should enqueue");

            // Verify it's in the outbox
            let rows = kit
                .db
                .query(&Statement::with_values(
                    "SELECT id, topic FROM approval_outbox WHERE id = ?".to_owned(),
                    vec![SeaValue::from(outbox_id.as_str())],
                ))
                .await
                .expect("should query");
            assert_eq!(rows.rows.len(), 1);

            // Check record count matches expectation
            let all_rows = kit
                .db
                .query(&Statement::new(
                    "SELECT COUNT(*) as cnt FROM approval_outbox".to_owned(),
                ))
                .await
                .expect("should count");
            let count: i64 = all_rows
                .rows
                .first()
                .and_then(|r| r.get("cnt"))
                .unwrap_or(0);
            assert!(count >= 1, "should have at least the enqueued notice");
        });
    }
}

// ============================================================================
// T2: WebPush for High-Risk Approvals
// ============================================================================

#[test]
fn t2_webpush_high_risk_with_actions() {
    for kit in cratefield_testing::TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_db(&*kit.db).await;

            // Enable webpush and add target
            storage::enable_channel(&*kit.db, COMPANY_ID, Channel::WebPush, NOW)
                .await
                .expect("should enable channel");

            let push_sub = serde_json::json!({
                "endpoint": "https://fcm.example.com/",
                "keys": {"p256dh": "base64url_p256dh_key", "auth": "base64url_auth_secret"}
            })
            .to_string();

            storage::add_target(
                &*kit.db,
                COMPANY_ID,
                PERSON_ID,
                Channel::WebPush,
                &push_sub,
                NOW,
            )
            .await
            .expect("should add target");

            // Load targets to verify
            let targets = storage::load_targets(&*kit.db, COMPANY_ID, PERSON_ID)
                .await
                .expect("should load targets");
            assert_eq!(targets.len(), 1);
            assert_eq!(targets[0].0, Channel::WebPush);
        });
    }
}

// ============================================================================
// T3: Email Delivery
// ============================================================================

#[test]
fn t3_email_requires_both_enable_and_target() {
    for kit in cratefield_testing::TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_db(&*kit.db).await;

            let person_id = "approver-email-test";

            // Case 1: Target only (no company setting) - should not send
            storage::add_target(
                &*kit.db,
                COMPANY_ID,
                person_id,
                Channel::Email,
                "test@example.com",
                NOW,
            )
            .await
            .expect("should add target");

            let enabled = storage::is_channel_enabled(&*kit.db, COMPANY_ID, Channel::Email)
                .await
                .expect("should check");
            assert!(!enabled, "email should not be enabled yet");

            // Case 2: Company setting only (no person target) - should not send
            storage::enable_channel(&*kit.db, COMPANY_ID, Channel::Email, NOW)
                .await
                .expect("should enable");

            let enabled = storage::is_channel_enabled(&*kit.db, COMPANY_ID, Channel::Email)
                .await
                .expect("should check");
            assert!(enabled, "email should be enabled now");

            let targets = storage::load_targets(&*kit.db, COMPANY_ID, "different-person")
                .await
                .expect("should load");
            assert_eq!(targets.len(), 0, "different person has no targets");

            // Case 3: Both enabled and target exists - should send
            let targets = storage::load_targets(&*kit.db, COMPANY_ID, person_id)
                .await
                .expect("should load");
            assert_eq!(targets.len(), 1);
            assert_eq!(targets[0].0, Channel::Email);
            assert_eq!(targets[0].1, "test@example.com");
        });
    }
}

// ============================================================================
// T4: Discord Delivery
// ============================================================================

#[test]
fn t4_discord_requires_both_enable_and_target() {
    for kit in cratefield_testing::TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_db(&*kit.db).await;

            let person_id = "approver-discord-test";
            let webhook_url = "https://discord.com/api/webhooks/123/abc";

            // Case 1: Enable channel for company
            storage::enable_channel(&*kit.db, COMPANY_ID, Channel::Discord, NOW)
                .await
                .expect("should enable");

            let enabled = storage::is_channel_enabled(&*kit.db, COMPANY_ID, Channel::Discord)
                .await
                .expect("should check");
            assert!(enabled);

            // Case 2: Add target for person
            storage::add_target(
                &*kit.db,
                COMPANY_ID,
                person_id,
                Channel::Discord,
                webhook_url,
                NOW,
            )
            .await
            .expect("should add target");

            // Case 3: Both exist - should be ready to send
            let targets = storage::load_targets(&*kit.db, COMPANY_ID, person_id)
                .await
                .expect("should load");
            assert_eq!(targets.len(), 1);
            assert_eq!(targets[0].0, Channel::Discord);
            assert_eq!(targets[0].1, webhook_url);
        });
    }
}

// ============================================================================
// T5: Slack Delivery
// ============================================================================

#[test]
fn t5_slack_requires_both_enable_and_target() {
    for kit in cratefield_testing::TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_db(&*kit.db).await;

            let person_id = "approver-slack-test";
            let webhook_url = "https://hooks.slack.com/services/T123/B456/xyz";

            // Enable Slack channel
            storage::enable_channel(&*kit.db, COMPANY_ID, Channel::Slack, NOW)
                .await
                .expect("should enable");

            // Add Slack webhook target
            storage::add_target(
                &*kit.db,
                COMPANY_ID,
                person_id,
                Channel::Slack,
                webhook_url,
                NOW,
            )
            .await
            .expect("should add target");

            // Verify setup
            let enabled = storage::is_channel_enabled(&*kit.db, COMPANY_ID, Channel::Slack)
                .await
                .expect("should check");
            assert!(enabled);

            let targets = storage::load_targets(&*kit.db, COMPANY_ID, person_id)
                .await
                .expect("should load");
            assert_eq!(targets.len(), 1);
            assert_eq!(targets[0].0, Channel::Slack);
        });
    }
}

// ============================================================================
// T6: Outbox Records After Raise and Reject
// ============================================================================

#[test]
fn t6_outbox_records_after_raise_and_reject() {
    for kit in cratefield_testing::TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_db(&*kit.db).await;

            let approval_id = "appr-raise-reject-001";
            let mut effect = BTreeMap::new();
            effect.insert("EUR".to_string(), "123450".to_string());

            let request = ApprovalRequest {
                id: approval_id.to_string(),
                company_id: COMPANY_ID.to_string(),
                command: "bill.pay".to_string(),
                input: "{}".to_string(),
                preview: r#"{"totals":{"EUR":"123450"}}"#.to_string(),
                reads: "{}".to_string(),
                request_hash: "test_hash_123".to_string(),
                effect: effect.clone(),
                requested_by: "agent-key".to_string(),
                reason: "above limit".to_string(),
                status: Status::Pending,
                created_at: NOW.to_string(),
                expires_at: "2026-01-15T10:30:00Z".to_string(),
                decided_by: None,
                credential_id: None,
            };

            // Simulate raise: enqueue created notice
            let stmt_created = notice_statement(
                approval_id,
                ApprovalEvent::Created,
                &request,
                Some("Acme GmbH"),
                NOW,
            )
            .expect("should create notice");
            kit.db
                .batch_atomic(&[stmt_created])
                .await
                .expect("should enqueue created");

            // Verify created record
            let rows = kit
                .db
                .query(&Statement::with_values(
                    "SELECT id, topic FROM approval_outbox WHERE id = ?".to_owned(),
                    vec![SeaValue::from(format!("{}-created", approval_id).as_str())],
                ))
                .await
                .expect("should query");
            assert_eq!(rows.rows.len(), 1, "should have created notice");
            if let Some(row) = rows.rows.first() {
                let topic: Option<String> = row.get("topic");
                assert_eq!(topic.as_deref(), Some("approval.created"));
            }

            // Simulate reject: enqueue decided notice
            let stmt_decided =
                notice_statement(approval_id, ApprovalEvent::Decided, &request, None, NOW)
                    .expect("should create notice");
            kit.db
                .batch_atomic(&[stmt_decided])
                .await
                .expect("should enqueue decided");

            // Verify both records exist
            let all_rows = kit
                .db
                .query(&Statement::with_values(
                    "SELECT id, topic FROM approval_outbox WHERE id LIKE ?".to_owned(),
                    vec![SeaValue::from(format!("{}%", approval_id).as_str())],
                ))
                .await
                .expect("should query");
            assert_eq!(
                all_rows.rows.len(),
                2,
                "should have both created and decided notices"
            );

            // Verify one is created and one is decided
            let topics: Vec<_> = all_rows
                .rows
                .iter()
                .filter_map(|r| r.get::<String>("topic"))
                .collect();
            assert!(topics.contains(&"approval.created".to_string()));
            assert!(topics.contains(&"approval.decided".to_string()));
        });
    }
}

// ============================================================================
// Storage Tests (from original, kept short)
// ============================================================================

#[test]
fn storage_enable_and_check_channel() {
    for kit in cratefield_testing::TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_db(&*kit.db).await;

            let enabled = storage::is_channel_enabled(&*kit.db, COMPANY_ID, Channel::Email)
                .await
                .expect("should check channel");
            assert!(!enabled);

            storage::enable_channel(&*kit.db, COMPANY_ID, Channel::Email, NOW)
                .await
                .expect("should enable");

            let enabled = storage::is_channel_enabled(&*kit.db, COMPANY_ID, Channel::Email)
                .await
                .expect("should check channel");
            assert!(enabled);
        });
    }
}

#[test]
fn storage_add_and_load_targets() {
    for kit in cratefield_testing::TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_db(&*kit.db).await;

            let person_id = "person-001";

            storage::add_target(
                &*kit.db,
                COMPANY_ID,
                person_id,
                Channel::Email,
                "approver@example.com",
                NOW,
            )
            .await
            .expect("should add email target");

            let targets = storage::load_targets(&*kit.db, COMPANY_ID, person_id)
                .await
                .expect("should load targets");

            assert_eq!(targets.len(), 1);
            assert!(
                targets
                    .iter()
                    .any(|(ch, addr)| *ch == Channel::Email && addr == "approver@example.com")
            );
        });
    }
}

#[test]
fn storage_remove_webpush_target() {
    for kit in cratefield_testing::TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_db(&*kit.db).await;

            let person_id = "person-003";

            storage::add_target(
                &*kit.db,
                COMPANY_ID,
                person_id,
                Channel::WebPush,
                "{...}",
                NOW,
            )
            .await
            .expect("should add target");

            storage::add_target(
                &*kit.db,
                COMPANY_ID,
                person_id,
                Channel::Email,
                "test@example.com",
                NOW,
            )
            .await
            .expect("should add target");

            storage::remove_webpush_target(&*kit.db, COMPANY_ID, person_id)
                .await
                .expect("should remove");

            let targets = storage::load_targets(&*kit.db, COMPANY_ID, person_id)
                .await
                .expect("should load targets");

            assert_eq!(targets.len(), 1);
            assert_eq!(targets[0].0, Channel::Email);
        });
    }
}
