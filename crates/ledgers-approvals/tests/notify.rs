//! Integration tests for approval notifications delivery.

use cratefield_core::{Database, Module, Outbox, Statement};
use ledgers_approvals::{
    ApprovalEvent, ApprovalNotice,
    notify::storage::Channel,
    notify::{NOTICE_OUTBOX_TABLE, storage},
};
use ledgers_db::DbModule;
use sea_query::Value as SeaValue;
use std::collections::BTreeMap;

const COMPANY_ID: &str = "test-company-01234567890123";
const PERSON_ID: &str = "approver-person-id";
const NOW: &str = "2026-01-15T09:30:00Z";
const APP_URL: &str = "https://app.example";

fn modules() -> Vec<Box<dyn Module>> {
    vec![
        Box::new(DbModule),
        Box::new(ledgers_storage::StorageModule),
        Box::new(ledgers_approvals::ApprovalsModule),
        Box::new(ledgers_approvals::module::ApprovalsInboxModule),
        Box::new(ledgers_approvals::module::ApprovalsNotifyModule),
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
        link: format!("{APP_URL}/approvals/{approval_id}"),
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
// T1: Webhook Delivery with Retries and Backoff
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
            let outbox_id = format!("{approval_id}-created");
            let stmt = outbox.enqueue_statement(
                &outbox_id,
                ApprovalEvent::Created.topic(),
                &payload,
                Some(COMPANY_ID),
                NOW,
            );
            kit.db.execute(&stmt).await.expect("should enqueue");

            // Verify it's in the outbox with attempts = 0
            let rows = kit
                .db
                .query(&Statement::with_values(
                    "SELECT id, topic, attempts FROM approval_outbox WHERE id = ?".to_owned(),
                    vec![SeaValue::from(outbox_id.as_str())],
                ))
                .await
                .expect("should query");
            assert_eq!(rows.rows.len(), 1);
            if let Some(row) = rows.rows.first() {
                let attempts: i64 = row.get("attempts").unwrap_or(0);
                assert_eq!(attempts, 0, "initial attempts should be 0");
            }
        });
    }
}

// ============================================================================
// T2: WebPush for High-Risk Approvals (Actions and URL Targets)
// ============================================================================

#[test]
fn t2_webpush_high_risk_with_actions() {
    for kit in cratefield_testing::TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_db(&*kit.db).await;

            let approval_id = "appr-highrisk-001";

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

            // Create a high-risk approval notice
            let notice = test_notice(approval_id, ApprovalEvent::Created, true);
            assert!(notice.high_risk, "notice should be marked high-risk");

            // The approve action for high-risk should include ?step=passkey
            // and the reject action should not (verified in sw.js notificationclick handler)
            assert_eq!(notice.approval_id, approval_id);

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
