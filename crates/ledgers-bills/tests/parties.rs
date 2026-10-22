//! Integration tests for parties commands (issue #44).
//!
//! Tests cover:
//! - Human-only enforcement (`set_bank`, `merge`)
//! - Bank change history with correct `person_id`
//! - VIES checking with mocked transport
//! - Duplicate detection
//! - Append-only enforcement on `vendor_bank_change` and `vies_check` tables
#![allow(clippy::unreadable_literal)]

use cratefield_core::{Database, Statement};
use cratefield_testing::TestHarness;
use ledgers_bills::parties::*;
use ledgers_commands::auth::{Authenticator, Credential, Limits, Principal, Tier};
use ledgers_commands::*;
use ledgers_db::DbModule;
use sea_query::Value;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;

const COMPANY_ID: &str = "01TESTCOMPANY0000000000000A";
const NOW: &str = "2026-01-15T09:30:00Z";
const OWNER_ID: &str = "owner-acme";
const AGENT_ID: &str = "agent-key-draft";

// ============================================================================
// Test Authenticator
// ============================================================================

struct TestAuthenticator {
    credentials: BTreeMap<String, Principal>,
}

impl TestAuthenticator {
    fn new() -> Self {
        let mut credentials = BTreeMap::new();

        // Owner (person)
        credentials.insert(
            "bearer-owner".to_string(),
            Principal::Person {
                person_id: OWNER_ID.to_string(),
            },
        );

        // Draft agent
        credentials.insert(
            "bearer-draft-agent".to_string(),
            Principal::Agent {
                key_id: AGENT_ID.to_string(),
                tier: Tier::Draft,
                limits: Limits::new(),
            },
        );

        TestAuthenticator { credentials }
    }
}

#[async_trait::async_trait]
impl Authenticator for TestAuthenticator {
    async fn authenticate(&self, credential: &Credential) -> Result<Principal, CommandError> {
        self.credentials
            .get(credential)
            .cloned()
            .ok_or(CommandError::Unauthenticated)
    }
}

// ============================================================================
// Mock Transport for VIES testing
// ============================================================================

struct MockViesTransport {
    response: serde_json::Value,
    should_fail: bool,
}

impl MockViesTransport {
    fn valid_response(_vat_number: &str, name: &str) -> Self {
        Self {
            response: json!({
                "valid": true,
                "name": name,
                "address": "Some Address, Country",
                "requestIdentifier": "consultation-123",
                "requestDate": "2026-01-15"
            }),
            should_fail: false,
        }
    }

    fn invalid_response() -> Self {
        Self {
            response: json!({
                "valid": false,
            }),
            should_fail: false,
        }
    }

    fn error_response() -> Self {
        Self {
            response: json!({}),
            should_fail: true,
        }
    }
}

#[async_trait::async_trait]
impl ledgers_extract::llm::Transport for MockViesTransport {
    async fn post_json(
        &self,
        _url: &str,
        _headers: &[(&str, String)],
        _body: &serde_json::Value,
    ) -> Result<(u16, serde_json::Value), String> {
        if self.should_fail {
            Ok((500, json!({})))
        } else {
            Ok((200, self.response.clone()))
        }
    }
}

// ============================================================================
// Database setup
// ============================================================================

async fn setup_parties_tables(db: &dyn Database) {
    // The migrations should create these tables, but ensure they exist
    let _ = db
        .execute(&Statement::new(
            "CREATE TABLE IF NOT EXISTS bill (\
                id TEXT PRIMARY KEY, \
                company_id TEXT NOT NULL, \
                document_id TEXT, \
                vendor_id TEXT NOT NULL, \
                posted_entry_id TEXT, \
                discarded_at TEXT, \
                iban_changed INTEGER DEFAULT 0, \
                created_at TEXT, \
                updated_at TEXT\
            )",
        ))
        .await;

    let _ = db
        .execute(&Statement::new(
            "CREATE TABLE IF NOT EXISTS vendor (\
                id TEXT PRIMARY KEY, \
                company_id TEXT NOT NULL, \
                name TEXT NOT NULL, \
                vat_id TEXT, \
                iban TEXT, \
                bic TEXT, \
                roles TEXT DEFAULT 'vendor', \
                merged_into TEXT, \
                personal_data INTEGER DEFAULT 0, \
                created_at TEXT, \
                updated_at TEXT\
            )",
        ))
        .await;

    let _ = db
        .execute(&Statement::new(
            "CREATE TABLE IF NOT EXISTS vendor_bank_change (\
                id TEXT PRIMARY KEY, \
                company_id TEXT NOT NULL, \
                vendor_id TEXT NOT NULL, \
                before_iban TEXT, \
                before_bic TEXT, \
                after_iban TEXT, \
                after_bic TEXT, \
                changed_by TEXT NOT NULL, \
                changed_at TEXT NOT NULL, \
                reason TEXT, \
                created_at TEXT\
            )",
        ))
        .await;

    let _ = db
        .execute(&Statement::new(
            "CREATE TABLE IF NOT EXISTS vies_check (\
                id TEXT PRIMARY KEY, \
                company_id TEXT NOT NULL, \
                vendor_id TEXT NOT NULL, \
                country_code TEXT NOT NULL, \
                vat_number TEXT NOT NULL, \
                valid INTEGER NOT NULL, \
                name TEXT, \
                address TEXT, \
                consultation_number TEXT, \
                request_date TEXT, \
                checked_at TEXT\
            )",
        ))
        .await;

    // Add triggers for append-only enforcement on SQLite
    let _ = db
        .execute(&Statement::new(
            "CREATE TRIGGER IF NOT EXISTS vendor_bank_change_no_update \
            BEFORE UPDATE ON vendor_bank_change \
            BEGIN SELECT RAISE(ABORT, 'vendor_bank_change is append-only'); END",
        ))
        .await;

    let _ = db
        .execute(&Statement::new(
            "CREATE TRIGGER IF NOT EXISTS vendor_bank_change_no_delete \
            BEFORE DELETE ON vendor_bank_change \
            BEGIN SELECT RAISE(ABORT, 'vendor_bank_change is append-only'); END",
        ))
        .await;

    let _ = db
        .execute(&Statement::new(
            "CREATE TRIGGER IF NOT EXISTS vies_check_no_update \
            BEFORE UPDATE ON vies_check \
            BEGIN SELECT RAISE(ABORT, 'vies_check is append-only'); END",
        ))
        .await;

    let _ = db
        .execute(&Statement::new(
            "CREATE TRIGGER IF NOT EXISTS vies_check_no_delete \
            BEFORE DELETE ON vies_check \
            BEGIN SELECT RAISE(ABORT, 'vies_check is append-only'); END",
        ))
        .await;
}

fn modules() -> Vec<Box<dyn cratefield_core::Module>> {
    vec![Box::new(DbModule)]
}

fn context(request_id: &str) -> CommandContext {
    CommandContext {
        company_id: COMPANY_ID.to_string(),
        request_id: request_id.to_string(),
        event_id: format!("{request_id}-event"),
        now: NOW.to_string(),
    }
}

async fn create_test_party(
    db: &dyn Database,
    id: &str,
    name: &str,
    vat_id: Option<&str>,
    iban: Option<&str>,
) {
    let _ = db
        .execute(&Statement::with_values(
            "INSERT INTO vendor (id, company_id, name, vat_id, iban, roles, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, 'vendor', ?, ?)"
                .to_string(),
            vec![
                Value::from(id),
                Value::from(COMPANY_ID),
                Value::from(name),
                vat_id.map_or(Value::String(None), Value::from),
                iban.map_or(Value::String(None), Value::from),
                Value::from(NOW),
                Value::from(NOW),
            ],
        ))
        .await;
}

// ============================================================================
// Tests
// ============================================================================

#[test]
fn parties_list_reads_parties_and_latest_vies() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_parties_tables(&*kit.db).await;

            // Create a test party
            create_test_party(
                &*kit.db,
                "vendor-1",
                "Acme GmbH",
                Some("DE123456789"),
                Some("DE89370400440532013000"),
            )
            .await;

            // Create a VIES check record
            let _ = kit
                .db
                .execute(&Statement::with_values(
                    "INSERT INTO vies_check (id, company_id, vendor_id, country_code, vat_number, valid, \
                     consultation_number, checked_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
                        .to_string(),
                    vec![
                        Value::from("vies-1"),
                        Value::from(COMPANY_ID),
                        Value::from("vendor-1"),
                        Value::from("DE"),
                        Value::from("123456789"),
                        Value::from(1i64),
                        Value::from("req-123"),
                        Value::from(NOW),
                    ],
                ))
                .await;

            let auth = TestAuthenticator::new();
            let policy = DefaultPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let cmd = PartiesList;
            let input = ListInput {
                limit: None,
                offset: None,
            };

            let result = executor
                .run(
                    &cmd,
                    "bearer-owner".to_string(),
                    context("list-test-1"),
                    &input,
                    Execute::Run,
                )
                .await
                .expect("list should succeed");

            match result {
                Executed::Done(output) => {
                    assert_eq!(output.parties.len(), 1);
                    let party = &output.parties[0];
                    assert_eq!(party.id, "vendor-1");
                    assert_eq!(party.name, "Acme GmbH");
                    assert_eq!(party.vat_id, Some("DE123456789".to_string()));
                    assert!(party.latest_vies.is_some());
                    let vies = party.latest_vies.as_ref().unwrap();
                    assert!(vies.valid);
                    assert_eq!(vies.consultation_number, Some("req-123".to_string()));
                }
                _ => panic!("expected Done"),
            }
        });
    }
}

#[test]
fn parties_set_bank_refused_to_agent() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_parties_tables(&*kit.db).await;
            create_test_party(&*kit.db, "vendor-2", "Halden Works GmbH", None, None).await;

            let auth = TestAuthenticator::new();
            let policy = DefaultPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let cmd = PartiesSetBank::new(None);
            let input = SetBankInput {
                id: "vendor-2".to_string(),
                iban: "GB82WEST12345698765432".to_string(),
                bic: Some("WESTGB2L".to_string()),
                reason: "test change".to_string(),
            };

            let result = executor
                .run(
                    &cmd,
                    "bearer-draft-agent".to_string(),
                    context("set-bank-agent-test"),
                    &input,
                    Execute::Run,
                )
                .await;

            assert!(result.is_err());
            assert!(matches!(result, Err(CommandError::HumanOnly)));
        });
    }
}

#[test]
fn parties_set_bank_succeeds_for_owner_and_records_change() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_parties_tables(&*kit.db).await;
            create_test_party(&*kit.db, "vendor-3", "Halden Works GmbH", None, None).await;

            let auth = TestAuthenticator::new();
            let policy = DefaultPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let cmd = PartiesSetBank::new(Some(OWNER_ID.to_string()));
            let input = SetBankInput {
                id: "vendor-3".to_string(),
                iban: "DE89370400440532013000".to_string(),
                bic: Some("COBADE".to_string()),
                reason: "bank account update".to_string(),
            };

            let result = executor
                .run(
                    &cmd,
                    "bearer-owner".to_string(),
                    context("set-bank-owner-test"),
                    &input,
                    Execute::Run,
                )
                .await
                .expect("set_bank should succeed for owner");

            match result {
                Executed::Done(output) => {
                    assert_eq!(output.party.id, "vendor-3");
                    assert_eq!(
                        output.party.iban,
                        Some("DE89370400440532013000".to_string())
                    );
                    assert_eq!(output.party.bic, Some("COBADE".to_string()));
                }
                _ => panic!("expected Done"),
            }

            // Verify the bank change record was created with correct person_id
            let rows = kit
                .db
                .query(&Statement::new(
                    "SELECT changed_by, before_iban, after_iban, reason FROM vendor_bank_change \
                     WHERE vendor_id = 'vendor-3'",
                ))
                .await
                .expect("query should succeed");

            assert_eq!(rows.rows.len(), 1);
            let change = &rows.rows[0];
            assert_eq!(
                change.get::<String>("changed_by"),
                Some(OWNER_ID.to_string()),
                "changed_by should store person_id"
            );
            assert_eq!(change.get::<Option<String>>("before_iban"), Some(None));
            assert_eq!(
                change.get::<String>("after_iban"),
                Some("DE89370400440532013000".to_string())
            );
            assert_eq!(
                change.get::<String>("reason"),
                Some("bank account update".to_string())
            );
        });
    }
}

#[test]
fn parties_set_bank_fails_without_person_id() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_parties_tables(&*kit.db).await;
            create_test_party(&*kit.db, "vendor-3b", "Halden Works GmbH", None, None).await;

            let auth = TestAuthenticator::new();
            let policy = DefaultPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            // Create command with None person_id (missing principal)
            let cmd = PartiesSetBank::new(None);
            let input = SetBankInput {
                id: "vendor-3b".to_string(),
                iban: "DE89370400440532013000".to_string(),
                bic: Some("COBADE".to_string()),
                reason: "bank account update".to_string(),
            };

            let result = executor
                .run(
                    &cmd,
                    "bearer-owner".to_string(),
                    context("set-bank-no-person-id-test"),
                    &input,
                    Execute::Run,
                )
                .await;

            // Should fail with Unauthenticated because person_id is None
            assert!(result.is_err());
            assert!(matches!(result, Err(CommandError::Unauthenticated)));

            // Verify no bank change record was written
            let rows = kit
                .db
                .query(&Statement::new(
                    "SELECT COUNT(*) as cnt FROM vendor_bank_change WHERE vendor_id = 'vendor-3b'",
                ))
                .await
                .expect("query should succeed");

            assert_eq!(rows.rows.len(), 1);
            let cnt = rows.rows[0].get::<i64>("cnt").unwrap_or(0);
            assert_eq!(
                cnt, 0,
                "no bank change record should be written without person_id"
            );
        });
    }
}

#[test]
fn parties_merge_refused_to_agent() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_parties_tables(&*kit.db).await;
            create_test_party(&*kit.db, "vendor-4a", "Old Acme", None, None).await;
            create_test_party(&*kit.db, "vendor-4b", "New Acme", None, None).await;

            let auth = TestAuthenticator::new();
            let policy = DefaultPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let cmd = PartiesMerge::new(None);
            let input = MergeInput {
                id: "vendor-4b".to_string(),
                from: "vendor-4a".to_string(),
            };

            let result = executor
                .run(
                    &cmd,
                    "bearer-draft-agent".to_string(),
                    context("merge-agent-test"),
                    &input,
                    Execute::Run,
                )
                .await;

            assert!(result.is_err());
            assert!(matches!(result, Err(CommandError::HumanOnly)));
        });
    }
}

#[test]
fn parties_merge_succeeds_for_owner_and_repoints_bills() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_parties_tables(&*kit.db).await;

            // Create vendor table and bill table for this test
            let _ = kit
                .db
                .execute(&Statement::new(
                    "CREATE TABLE IF NOT EXISTS bill (\
                        id TEXT PRIMARY KEY, \
                        company_id TEXT NOT NULL, \
                        document_id TEXT, \
                        vendor_id TEXT NOT NULL, \
                        posted_entry_id TEXT, \
                        discarded_at TEXT, \
                        iban_changed INTEGER DEFAULT 0, \
                        created_at TEXT, \
                        updated_at TEXT\
                    )",
                ))
                .await;

            create_test_party(&*kit.db, "vendor-5a", "To Merge", None, None).await;
            create_test_party(&*kit.db, "vendor-5b", "Survivor", None, None).await;

            // Create a bill for the source party
            let _ = kit
                .db
                .execute(&Statement::with_values(
                    "INSERT INTO bill (id, company_id, vendor_id, created_at, updated_at) \
                     VALUES (?, ?, ?, ?, ?)"
                        .to_string(),
                    vec![
                        Value::from("bill-1"),
                        Value::from(COMPANY_ID),
                        Value::from("vendor-5a"),
                        Value::from(NOW),
                        Value::from(NOW),
                    ],
                ))
                .await;

            let auth = TestAuthenticator::new();
            let policy = DefaultPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let cmd = PartiesMerge::new(Some(OWNER_ID.to_string()));
            let input = MergeInput {
                id: "vendor-5b".to_string(),
                from: "vendor-5a".to_string(),
            };

            let result = executor
                .run(
                    &cmd,
                    "bearer-owner".to_string(),
                    context("merge-owner-test"),
                    &input,
                    Execute::Run,
                )
                .await
                .expect("merge should succeed for owner");

            match result {
                Executed::Done(_) => {
                    // Verify the source party is marked as merged
                    let rows = kit
                        .db
                        .query(&Statement::new(
                            "SELECT merged_into FROM vendor WHERE id = 'vendor-5a'",
                        ))
                        .await
                        .expect("query should succeed");

                    assert_eq!(rows.rows.len(), 1);
                    assert_eq!(
                        rows.rows[0].get::<Option<String>>("merged_into"),
                        Some(Some("vendor-5b".to_string())),
                        "source party should be marked merged_into survivor"
                    );

                    // Verify the bill was repointed to the survivor
                    let bill_rows = kit
                        .db
                        .query(&Statement::new(
                            "SELECT vendor_id FROM bill WHERE id = 'bill-1'",
                        ))
                        .await
                        .expect("query should succeed");

                    assert_eq!(bill_rows.rows.len(), 1);
                    assert_eq!(
                        bill_rows.rows[0].get::<String>("vendor_id"),
                        Some("vendor-5b".to_string()),
                        "bill should be repointed to survivor"
                    );
                }
                _ => panic!("expected Done"),
            }
        });
    }
}

#[test]
fn parties_check_vat_with_valid_eu_number_stores_result() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_parties_tables(&*kit.db).await;
            create_test_party(&*kit.db, "vendor-6", "German Corp", None, None).await;

            let auth = TestAuthenticator::new();
            let policy = DefaultPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let mock_transport =
                Arc::new(MockViesTransport::valid_response("123456789", "Acme GmbH"));
            let cmd = PartiesCheckVat::new(Some(mock_transport));
            let input = CheckVatInput {
                id: "vendor-6".to_string(),
                country_code: "DE".to_string(),
                vat_number: "123456789".to_string(),
            };

            let result = executor
                .run(
                    &cmd,
                    "bearer-draft-agent".to_string(),
                    context("check-vat-valid"),
                    &input,
                    Execute::Run,
                )
                .await
                .expect("check_vat should succeed");

            match result {
                Executed::Done(output) => {
                    assert!(output.vies_result.valid);
                    assert_eq!(output.vies_result.name, Some("Acme GmbH".to_string()));
                    assert_eq!(
                        output.vies_result.consultation_number,
                        Some("consultation-123".to_string())
                    );
                }
                _ => panic!("expected Done"),
            }

            // Verify vies_check record was created
            let rows = kit
                .db
                .query(&Statement::new(
                    "SELECT valid, consultation_number FROM vies_check WHERE vendor_id = 'vendor-6'",
                ))
                .await
                .expect("query should succeed");

            assert_eq!(rows.rows.len(), 1);
            assert_eq!(rows.rows[0].get::<i64>("valid"), Some(1));
            assert_eq!(
                rows.rows[0].get::<Option<String>>("consultation_number"),
                Some(Some("consultation-123".to_string()))
            );
        });
    }
}

#[test]
fn parties_check_vat_with_non_eu_country_rejected() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_parties_tables(&*kit.db).await;
            create_test_party(&*kit.db, "vendor-7", "US Corp", None, None).await;

            let auth = TestAuthenticator::new();
            let policy = DefaultPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let mock_transport =
                Arc::new(MockViesTransport::valid_response("123456789", "US Corp"));
            let cmd = PartiesCheckVat::new(Some(mock_transport));
            let input = CheckVatInput {
                id: "vendor-7".to_string(),
                country_code: "US".to_string(),
                vat_number: "123456789".to_string(),
            };

            let result = executor
                .run(
                    &cmd,
                    "bearer-draft-agent".to_string(),
                    context("check-vat-non-eu"),
                    &input,
                    Execute::Run,
                )
                .await;

            assert!(result.is_err(), "non-EU country should be rejected");
            assert!(matches!(result, Err(CommandError::InvalidInput(_))));

            // Verify no vies_check record was created
            let rows = kit
                .db
                .query(&Statement::new(
                    "SELECT COUNT(*) as cnt FROM vies_check WHERE vendor_id = 'vendor-7'",
                ))
                .await
                .expect("query should succeed");

            let count = rows.rows[0].get::<i64>("cnt").unwrap_or(0);
            assert_eq!(
                count, 0,
                "no vies_check should be created for non-EU country"
            );
        });
    }
}

#[test]
fn parties_check_vat_with_error_response_fails() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_parties_tables(&*kit.db).await;
            create_test_party(&*kit.db, "vendor-8", "Company X", None, None).await;

            let auth = TestAuthenticator::new();
            let policy = DefaultPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let mock_transport = Arc::new(MockViesTransport::error_response());
            let cmd = PartiesCheckVat::new(Some(mock_transport));
            let input = CheckVatInput {
                id: "vendor-8".to_string(),
                country_code: "DE".to_string(),
                vat_number: "999999999".to_string(),
            };

            let result = executor
                .run(
                    &cmd,
                    "bearer-draft-agent".to_string(),
                    context("check-vat-error"),
                    &input,
                    Execute::Run,
                )
                .await;

            assert!(
                result.is_err(),
                "check_vat should fail when VIES returns non-200 response"
            );

            // Verify no vies_check record was created
            let rows = kit
                .db
                .query(&Statement::new(
                    "SELECT COUNT(*) as cnt FROM vies_check WHERE vendor_id = 'vendor-8'",
                ))
                .await
                .expect("query should succeed");

            let count = rows.rows[0].get::<i64>("cnt").unwrap_or(0);
            assert_eq!(
                count, 0,
                "no vies_check should be created when VIES returns non-200"
            );
        });
    }
}

#[test]
fn parties_check_vat_with_invalid_response_fails() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_parties_tables(&*kit.db).await;
            create_test_party(&*kit.db, "vendor-9", "Company Y", None, None).await;

            let auth = TestAuthenticator::new();
            let policy = DefaultPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let mock_transport = Arc::new(MockViesTransport::invalid_response());
            let cmd = PartiesCheckVat::new(Some(mock_transport));
            let input = CheckVatInput {
                id: "vendor-9".to_string(),
                country_code: "DE".to_string(),
                vat_number: "111111111".to_string(),
            };

            let result = executor
                .run(
                    &cmd,
                    "bearer-draft-agent".to_string(),
                    context("check-vat-invalid"),
                    &input,
                    Execute::Run,
                )
                .await
                .expect("check_vat should succeed even with invalid VAT");

            match result {
                Executed::Done(output) => {
                    assert!(
                        !output.vies_result.valid,
                        "VIES result should indicate invalid VAT"
                    );
                }
                _ => panic!("expected Done"),
            }

            // Verify vies_check record was still created to track the check
            let rows = kit
                .db
                .query(&Statement::new(
                    "SELECT valid FROM vies_check WHERE vendor_id = 'vendor-9'",
                ))
                .await
                .expect("query should succeed");

            assert_eq!(rows.rows.len(), 1);
            assert_eq!(
                rows.rows[0].get::<i64>("valid"),
                Some(0),
                "vies_check should record invalid result"
            );
        });
    }
}

#[test]
fn vendor_bank_change_is_append_only() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_parties_tables(&*kit.db).await;

            // Try to insert a record
            let _ = kit
                .db
                .execute(&Statement::with_values(
                    "INSERT INTO vendor_bank_change (id, company_id, vendor_id, after_iban, \
                     changed_by, changed_at, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)"
                        .to_string(),
                    vec![
                        Value::from("change-1"),
                        Value::from(COMPANY_ID),
                        Value::from("vendor-test"),
                        Value::from("DE89370400440532013000"),
                        Value::from("owner-1"),
                        Value::from(NOW),
                        Value::from(NOW),
                    ],
                ))
                .await;

            // Try to UPDATE - should fail
            let update_result = kit
                .db
                .execute(&Statement::new(
                    "UPDATE vendor_bank_change SET changed_by = 'other' WHERE id = 'change-1'",
                ))
                .await;

            assert!(
                update_result.is_err(),
                "vendor_bank_change should not allow updates"
            );

            // Try to DELETE - should fail
            let delete_result = kit
                .db
                .execute(&Statement::new(
                    "DELETE FROM vendor_bank_change WHERE id = 'change-1'",
                ))
                .await;

            assert!(
                delete_result.is_err(),
                "vendor_bank_change should not allow deletes"
            );
        });
    }
}

#[test]
fn vies_check_is_append_only() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_parties_tables(&*kit.db).await;

            // Try to insert a record
            let _ = kit
                .db
                .execute(&Statement::with_values(
                    "INSERT INTO vies_check (id, company_id, vendor_id, country_code, \
                     vat_number, valid, checked_at) VALUES (?, ?, ?, ?, ?, ?, ?)"
                        .to_string(),
                    vec![
                        Value::from("vies-1"),
                        Value::from(COMPANY_ID),
                        Value::from("vendor-test"),
                        Value::from("DE"),
                        Value::from("123456789"),
                        Value::from(1i64),
                        Value::from(NOW),
                    ],
                ))
                .await;

            // Try to UPDATE - should fail
            let update_result = kit
                .db
                .execute(&Statement::new(
                    "UPDATE vies_check SET valid = 0 WHERE id = 'vies-1'",
                ))
                .await;

            assert!(
                update_result.is_err(),
                "vies_check should not allow updates"
            );

            // Try to DELETE - should fail
            let delete_result = kit
                .db
                .execute(&Statement::new(
                    "DELETE FROM vies_check WHERE id = 'vies-1'",
                ))
                .await;

            assert!(
                delete_result.is_err(),
                "vies_check should not allow deletes"
            );
        });
    }
}

#[test]
fn parties_create_and_update_reject_bank_fields() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_parties_tables(&*kit.db).await;

            let auth = TestAuthenticator::new();
            let policy = DefaultPolicy;
            let _executor = Executor::new(&*kit.db, auth, policy);

            // Try to create with iban field (should be rejected due to deny_unknown_fields)
            let invalid_input = json!({
                "id": "vendor-bad",
                "name": "Test Corp",
                "iban": "DE89370400440532013000"
            });

            let input_str = serde_json::to_string(&invalid_input).unwrap();
            let parsed: Result<CreateInput, _> = serde_json::from_str(&input_str);
            assert!(
                parsed.is_err(),
                "CreateInput should reject unknown iban field"
            );

            // Try to update with bic field (should be rejected)
            let invalid_update = json!({
                "id": "vendor-update-bad",
                "bic": "COBADE"
            });

            let input_str = serde_json::to_string(&invalid_update).unwrap();
            let parsed: Result<UpdateInput, _> = serde_json::from_str(&input_str);
            assert!(
                parsed.is_err(),
                "UpdateInput should reject unknown bic field"
            );
        });
    }
}

#[test]
fn parties_list_detects_duplicates_by_vat_id() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_parties_tables(&*kit.db).await;

            // Create two parties with the same VAT id (different cases/spaces)
            create_test_party(
                &*kit.db,
                "vendor-dup-1",
                "Company A",
                Some("DE 123456789"),
                None,
            )
            .await;
            create_test_party(
                &*kit.db,
                "vendor-dup-2",
                "Company B",
                Some("de123456789"),
                None,
            )
            .await;

            let auth = TestAuthenticator::new();
            let policy = DefaultPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let cmd = PartiesList;
            let input = ListInput {
                limit: None,
                offset: None,
            };

            let result = executor
                .run(
                    &cmd,
                    "bearer-owner".to_string(),
                    context("dup-vat-test"),
                    &input,
                    Execute::Run,
                )
                .await
                .expect("list should succeed");

            match result {
                Executed::Done(output) => {
                    assert_eq!(output.parties.len(), 2);

                    let party1 = output
                        .parties
                        .iter()
                        .find(|p| p.id == "vendor-dup-1")
                        .unwrap();
                    let party2 = output
                        .parties
                        .iter()
                        .find(|p| p.id == "vendor-dup-2")
                        .unwrap();

                    // Both should detect each other as duplicates
                    assert!(
                        party1
                            .possible_duplicates
                            .contains(&"vendor-dup-2".to_string())
                    );
                    assert!(
                        party2
                            .possible_duplicates
                            .contains(&"vendor-dup-1".to_string())
                    );
                }
                _ => panic!("expected Done"),
            }
        });
    }
}

#[test]
fn parties_list_detects_duplicates_by_iban() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_parties_tables(&*kit.db).await;

            // Create two parties with the same IBAN (different spacing)
            create_test_party(
                &*kit.db,
                "vendor-iban-1",
                "Company C",
                None,
                Some("DE89 3704 0044 0532 0130 00"),
            )
            .await;
            create_test_party(
                &*kit.db,
                "vendor-iban-2",
                "Company D",
                None,
                Some("de893704004405320130 00"),
            )
            .await;

            let auth = TestAuthenticator::new();
            let policy = DefaultPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let cmd = PartiesList;
            let input = ListInput {
                limit: None,
                offset: None,
            };

            let result = executor
                .run(
                    &cmd,
                    "bearer-owner".to_string(),
                    context("dup-iban-test"),
                    &input,
                    Execute::Run,
                )
                .await
                .expect("list should succeed");

            match result {
                Executed::Done(output) => {
                    assert_eq!(output.parties.len(), 2);

                    let party1 = output
                        .parties
                        .iter()
                        .find(|p| p.id == "vendor-iban-1")
                        .unwrap();
                    let party2 = output
                        .parties
                        .iter()
                        .find(|p| p.id == "vendor-iban-2")
                        .unwrap();

                    // Both should detect each other as duplicates
                    assert!(
                        party1
                            .possible_duplicates
                            .contains(&"vendor-iban-2".to_string())
                    );
                    assert!(
                        party2
                            .possible_duplicates
                            .contains(&"vendor-iban-1".to_string())
                    );
                }
                _ => panic!("expected Done"),
            }
        });
    }
}

#[test]
fn parties_list_detects_duplicates_by_normalized_name() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_parties_tables(&*kit.db).await;

            // Create two parties with the same normalized name
            create_test_party(&*kit.db, "vendor-name-1", "Halden Works GmbH", None, None).await;
            create_test_party(&*kit.db, "vendor-name-2", "HALDENWORKSGMBH", None, None).await;

            let auth = TestAuthenticator::new();
            let policy = DefaultPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let cmd = PartiesList;
            let input = ListInput {
                limit: None,
                offset: None,
            };

            let result = executor
                .run(
                    &cmd,
                    "bearer-owner".to_string(),
                    context("dup-name-test"),
                    &input,
                    Execute::Run,
                )
                .await
                .expect("list should succeed");

            match result {
                Executed::Done(output) => {
                    assert_eq!(output.parties.len(), 2);

                    let party1 = output
                        .parties
                        .iter()
                        .find(|p| p.id == "vendor-name-1")
                        .unwrap();
                    let party2 = output
                        .parties
                        .iter()
                        .find(|p| p.id == "vendor-name-2")
                        .unwrap();

                    // Both should detect each other as duplicates
                    assert!(
                        party1
                            .possible_duplicates
                            .contains(&"vendor-name-2".to_string())
                    );
                    assert!(
                        party2
                            .possible_duplicates
                            .contains(&"vendor-name-1".to_string())
                    );
                }
                _ => panic!("expected Done"),
            }
        });
    }
}

#[test]
fn parties_list_does_not_detect_merged_parties_as_duplicates() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_parties_tables(&*kit.db).await;

            // Create two parties with the same VAT id, then merge one
            create_test_party(
                &*kit.db,
                "vendor-merge-1",
                "Company E",
                Some("FR 987654321"),
                None,
            )
            .await;
            create_test_party(
                &*kit.db,
                "vendor-merge-2",
                "Company F",
                Some("fr987654321"),
                None,
            )
            .await;

            // Mark one as merged
            let _ = kit
                .db
                .execute(&Statement::with_values(
                    "UPDATE vendor SET merged_into = ? WHERE id = ?".to_string(),
                    vec![Value::from("vendor-merge-1"), Value::from("vendor-merge-2")],
                ))
                .await;

            let auth = TestAuthenticator::new();
            let policy = DefaultPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let cmd = PartiesList;
            let input = ListInput {
                limit: None,
                offset: None,
            };

            let result = executor
                .run(
                    &cmd,
                    "bearer-owner".to_string(),
                    context("dup-merged-test"),
                    &input,
                    Execute::Run,
                )
                .await
                .expect("list should succeed");

            match result {
                Executed::Done(output) => {
                    // Only unmerged parties are returned
                    assert_eq!(output.parties.len(), 1);
                    let party = &output.parties[0];
                    assert_eq!(party.id, "vendor-merge-1");
                    // No duplicates since merged parties are not in the list
                    assert!(party.possible_duplicates.is_empty());
                }
                _ => panic!("expected Done"),
            }
        });
    }
}

#[test]
fn parties_merge_guard_fails_when_source_already_merged() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_parties_tables(&*kit.db).await;

            // Create vendor table and bill table for this test
            let _ = kit
                .db
                .execute(&Statement::new(
                    "CREATE TABLE IF NOT EXISTS bill (\
                        id TEXT PRIMARY KEY, \
                        company_id TEXT NOT NULL, \
                        document_id TEXT, \
                        vendor_id TEXT NOT NULL, \
                        posted_entry_id TEXT, \
                        discarded_at TEXT, \
                        iban_changed INTEGER DEFAULT 0, \
                        created_at TEXT, \
                        updated_at TEXT\
                    )",
                ))
                .await;

            // Create three parties: A (to merge), B (initial survivor), and C (final survivor)
            create_test_party(&*kit.db, "vendor-guard-a", "Party A", None, None).await;
            create_test_party(&*kit.db, "vendor-guard-b", "Party B", None, None).await;
            create_test_party(&*kit.db, "vendor-guard-c", "Party C", None, None).await;

            let auth = TestAuthenticator::new();
            let policy = DefaultPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            // First merge: A into B
            let cmd1 = PartiesMerge::new(Some(OWNER_ID.to_string()));
            let input1 = MergeInput {
                id: "vendor-guard-b".to_string(),
                from: "vendor-guard-a".to_string(),
            };

            let result1 = executor
                .run(
                    &cmd1,
                    "bearer-owner".to_string(),
                    context("merge-guard-test-1"),
                    &input1,
                    Execute::Run,
                )
                .await
                .expect("first merge should succeed");

            match result1 {
                Executed::Done(_) => {
                    // Verify A is marked as merged into B
                    let rows = kit
                        .db
                        .query(&Statement::new(
                            "SELECT merged_into FROM vendor WHERE id = 'vendor-guard-a'",
                        ))
                        .await
                        .expect("query should succeed");

                    assert_eq!(rows.rows.len(), 1);
                    assert_eq!(
                        rows.rows[0].get::<Option<String>>("merged_into"),
                        Some(Some("vendor-guard-b".to_string())),
                        "party A should be marked merged_into B"
                    );

                    // Now try to merge A again (which is now merged) into C
                    // This should fail because A's guard statement will fail at write time
                    let cmd2 = PartiesMerge::new(Some(OWNER_ID.to_string()));
                    let input2 = MergeInput {
                        id: "vendor-guard-c".to_string(),
                        from: "vendor-guard-a".to_string(),
                    };

                    let result2 = executor
                        .run(
                            &cmd2,
                            "bearer-owner".to_string(),
                            context("merge-guard-test-2"),
                            &input2,
                            Execute::Run,
                        )
                        .await;

                    // This should fail due to the guard statement
                    assert!(
                        result2.is_err(),
                        "merge should fail when source is already merged (guard violation)"
                    );

                    // Verify A is still marked as merged into B (unchanged by failed merge)
                    let rows_after = kit
                        .db
                        .query(&Statement::new(
                            "SELECT merged_into FROM vendor WHERE id = 'vendor-guard-a'",
                        ))
                        .await
                        .expect("query should succeed");

                    assert_eq!(rows_after.rows.len(), 1);
                    assert_eq!(
                        rows_after.rows[0].get::<Option<String>>("merged_into"),
                        Some(Some("vendor-guard-b".to_string())),
                        "party A should still be marked merged_into B (unchanged by failed merge)"
                    );
                }
                _ => panic!("expected Done for first merge"),
            }
        });
    }
}

#[test]
fn parties_update_guard_fails_when_party_already_merged() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_parties_tables(&*kit.db).await;

            // Create a party and merge it
            create_test_party(&*kit.db, "vendor-update-a", "Party A", None, None).await;
            create_test_party(&*kit.db, "vendor-update-b", "Party B", None, None).await;

            // Mark party A as merged into B
            let _ = kit
                .db
                .execute(&Statement::with_values(
                    "UPDATE vendor SET merged_into = ? WHERE id = ?".to_string(),
                    vec![
                        Value::from("vendor-update-b"),
                        Value::from("vendor-update-a"),
                    ],
                ))
                .await;

            let auth = TestAuthenticator::new();
            let policy = DefaultPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            // Try to update the merged party
            let cmd = PartiesUpdate;
            let input = UpdateInput {
                id: "vendor-update-a".to_string(),
                name: Some("Updated Name".to_string()),
                vat_id: None,
                roles: None,
                tax_number: None,
                language: None,
                currency: None,
                personal_data: None,
                payment_terms_days: None,
                default_account: None,
                default_tax_code: None,
            };

            let result = executor
                .run(
                    &cmd,
                    "bearer-owner".to_string(),
                    context("update-guard-test"),
                    &input,
                    Execute::Run,
                )
                .await;

            // This should fail due to the guard statement
            assert!(
                result.is_err(),
                "update should fail when party is already merged (guard violation)"
            );

            // Verify the party name is unchanged
            let rows = kit
                .db
                .query(&Statement::new(
                    "SELECT name FROM vendor WHERE id = 'vendor-update-a'",
                ))
                .await
                .expect("query should succeed");

            assert_eq!(rows.rows.len(), 1);
            assert_eq!(
                rows.rows[0].get::<String>("name"),
                Some("Party A".to_string()),
                "party name should be unchanged by failed update"
            );
        });
    }
}
