//! Integration tests for the approvals service with passkey step-up.

use cratefield_core::{Database, Statement};
use cratefield_testing::TestHarness;
use ledgers_approvals::{
    ApprovalConfig, ApprovalError, Approvals, CredentialKeys, Es256Verifier, NoCredentialKeys,
    OwnerApprovesAll, PasskeyProof, get_request, list_requests,
};
use ledgers_commands::{
    Authenticator, Command, CommandContext, Credential, Execute, Executed, Executor,
    JournalPreview, PendingApproval, Plan, Policy, Principal, Tier, auth::Limits,
};
use ledgers_db::DbModule;
use ledgers_money::{Amount, Currency};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const COMPANY_ID: &str = "test-company-01234567890123";
const NOW: &str = "2026-01-15T09:30:00Z";
const OWNER_PERSON_ID: &str = "owner-person-id";
const AGENT_KEY_ID: &str = "agent-key-id";

// ============================================================================
// Test Bill Table and PayBill Command
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
struct PayBillInput {
    bill_id: String,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct PayBillOutput {
    success: bool,
}

struct PayBillCommand;

#[async_trait::async_trait]
impl Command for PayBillCommand {
    const NAME: &'static str = "bill.pay";
    const TIER: Tier = Tier::PostWithinLimits;
    const HUMAN_ONLY: bool = false;
    const SUPPORTS_DRY_RUN: bool = true;
    const IDEMPOTENT: bool = false;

    type Input = PayBillInput;
    type Output = PayBillOutput;

    async fn preview(
        &self,
        db: &dyn Database,
        _ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, ledgers_commands::CommandError> {
        let rows = db
            .query(&Statement::with_values(
                "SELECT amount_minor, currency FROM test_bill WHERE id = ?".to_owned(),
                vec![sea_query::Value::from(input.bill_id.as_str())],
            ))
            .await
            .map_err(|e| {
                ledgers_commands::CommandError::InvalidInput(format!("failed to load bill: {e}"))
            })?;

        let row = rows
            .first()
            .ok_or(ledgers_commands::CommandError::InvalidInput(
                "bill not found".to_string(),
            ))?;

        let amount_minor: i64 =
            row.get("amount_minor")
                .ok_or(ledgers_commands::CommandError::InvalidInput(
                    "missing amount".to_string(),
                ))?;
        let currency_code: String =
            row.get("currency")
                .ok_or(ledgers_commands::CommandError::InvalidInput(
                    "missing currency".to_string(),
                ))?;

        let currency = Currency::from_code(&currency_code).map_err(|_| {
            ledgers_commands::CommandError::InvalidInput("invalid currency".to_string())
        })?;
        let amount = Amount::new(i128::from(amount_minor), currency);

        let mut preview = JournalPreview::empty();
        preview
            .add_line("1000".to_string(), "debit", amount, None)
            .map_err(ledgers_commands::CommandError::from)?;

        Ok(preview)
    }

    async fn reads(
        &self,
        db: &dyn Database,
        _ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<BTreeMap<String, String>, ledgers_commands::CommandError> {
        let rows = db
            .query(&Statement::with_values(
                "SELECT version FROM test_bill WHERE id = ?".to_owned(),
                vec![sea_query::Value::from(input.bill_id.as_str())],
            ))
            .await
            .map_err(|e| {
                ledgers_commands::CommandError::InvalidInput(format!("failed to load bill: {e}"))
            })?;

        if let Some(row) = rows.first() {
            let version: i64 = row.get("version").unwrap_or(0);
            let mut reads = BTreeMap::new();
            reads.insert(format!("bill:{}", input.bill_id), version.to_string());
            Ok(reads)
        } else {
            Ok(BTreeMap::new())
        }
    }

    async fn plan(
        &self,
        _token: &ledgers_commands::ExecToken,
        _db: &dyn Database,
        _ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, ledgers_commands::CommandError> {
        let stmt = Statement::with_values(
            "UPDATE test_bill SET paid = 1 WHERE id = ?".to_owned(),
            vec![sea_query::Value::from(input.bill_id.as_str())],
        );

        Ok(Plan {
            statements: vec![stmt],
            output: PayBillOutput { success: true },
            objects: vec![format!("bill:{}", input.bill_id)],
        })
    }
}

// ============================================================================
// Test Authenticator and Policy
// ============================================================================

struct TestAuthenticator;

#[async_trait::async_trait]
impl Authenticator for TestAuthenticator {
    async fn authenticate(
        &self,
        credential: &Credential,
    ) -> Result<Principal, ledgers_commands::CommandError> {
        if credential == "owner" {
            Ok(Principal::Person {
                person_id: OWNER_PERSON_ID.to_string(),
                role: ledgers_commands::Role::Owner,
            })
        } else if credential == "agent" {
            Ok(Principal::Agent {
                key_id: AGENT_KEY_ID.to_string(),
                tier: Tier::PostWithinLimits,
                limits: Limits::new(),
            })
        } else {
            Err(ledgers_commands::CommandError::Unauthenticated)
        }
    }
}

struct TestPolicy;

impl Policy for TestPolicy {
    fn decide(
        &self,
        principal: &Principal,
        _meta: &ledgers_commands::CommandMeta,
        _preview: &JournalPreview,
    ) -> ledgers_commands::Decision {
        match principal {
            Principal::Person { .. } => ledgers_commands::Decision::Allow,
            Principal::Agent { .. } => ledgers_commands::Decision::NeedsApproval {
                reason: "agent requires approval".to_string(),
            },
        }
    }
}

// ============================================================================
// Virtual Authenticator for WebAuthn
// ============================================================================

struct VirtualAuthenticator {
    signer: p256::ecdsa::SigningKey,
}

impl VirtualAuthenticator {
    fn new_with_fixed_seed() -> Self {
        use p256::SecretKey;
        let seed = [42u8; 32];
        let secret = SecretKey::from_slice(&seed).expect("valid seed");
        let signer = p256::ecdsa::SigningKey::from(secret);
        VirtualAuthenticator { signer }
    }

    fn public_key_sec1_uncompressed(&self) -> Vec<u8> {
        let verifying_key = p256::ecdsa::VerifyingKey::from(&self.signer);
        verifying_key.to_encoded_point(false).as_bytes().to_vec()
    }

    fn assert(&self, challenge: &[u8], rp_id: &str, origin: &str, flags: u8) -> PasskeyProof {
        use p256::ecdsa::signature::Signer;
        use sha2::{Digest, Sha256};

        let rp_id_hash = Sha256::new().chain_update(rp_id.as_bytes()).finalize();
        let mut auth_data = rp_id_hash.to_vec();
        auth_data.push(flags);
        auth_data.extend_from_slice(&1u32.to_be_bytes());

        let client_data = serde_json::json!({
            "type": "webauthn.get",
            "challenge": base64_url_no_pad(challenge),
            "origin": origin,
        });
        let client_data_json = serde_json::to_vec(&client_data).unwrap();
        let client_data_hash = Sha256::new().chain_update(&client_data_json).finalize();

        let mut message = auth_data.clone();
        message.extend_from_slice(client_data_hash.as_slice());
        let signature: p256::ecdsa::Signature = self.signer.sign(&message);

        let public_key_bytes = self.public_key_sec1_uncompressed();

        PasskeyProof {
            credential_id: hex::encode(&public_key_bytes),
            client_data_json: base64_url_no_pad(&client_data_json),
            authenticator_data: base64_url_no_pad(&auth_data),
            signature: base64_url_no_pad(signature.to_der().as_ref()),
        }
    }
}

fn base64_url_no_pad(data: &[u8]) -> String {
    use base64::Engine;
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    engine.encode(data)
}

// ============================================================================
// Test Credential Keys
// ============================================================================

struct TestKeys {
    authenticator: VirtualAuthenticator,
}

impl TestKeys {
    fn new() -> Self {
        TestKeys {
            authenticator: VirtualAuthenticator::new_with_fixed_seed(),
        }
    }
}

impl CredentialKeys for TestKeys {
    fn public_key(&self, person_id: &str, _credential_id: &str) -> Option<Vec<u8>> {
        if person_id == OWNER_PERSON_ID {
            Some(self.authenticator.public_key_sec1_uncompressed())
        } else {
            None
        }
    }
}

// ============================================================================
// Test Setup Helpers
// ============================================================================

fn modules() -> Vec<Box<dyn cratefield_core::Module>> {
    vec![
        Box::new(DbModule),
        Box::new(ledgers_storage::StorageModule),
        Box::new(ledgers_approvals::ApprovalsModule),
        Box::new(ledgers_approvals::module::ApprovalsInboxModule),
        Box::new(ledgers_approvals::module::ApprovalsNotifyModule),
    ]
}

async fn setup_db(db: &dyn Database) {
    db.execute(&Statement::new(
        "CREATE TABLE IF NOT EXISTS test_bill (\
         id TEXT PRIMARY KEY,\
         amount_minor INTEGER NOT NULL,\
         currency TEXT NOT NULL,\
         version INTEGER NOT NULL DEFAULT 1,\
         paid INTEGER NOT NULL DEFAULT 0\
         )"
        .to_owned(),
    ))
    .await
    .expect("failed to create table");

    db.execute(&Statement::with_values(
        "INSERT INTO company (id, name, base_currency, created_at, updated_at) VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT (id) DO NOTHING"
            .to_owned(),
        vec![
            sea_query::Value::from(COMPANY_ID),
            sea_query::Value::from("Test Company"),
            sea_query::Value::from("EUR"),
            sea_query::Value::from(NOW),
            sea_query::Value::from(NOW),
        ],
    ))
    .await
    .ok();
}

async fn insert_bill(db: &dyn Database, id: &str, amount: i64, currency: &str) {
    db.execute(&Statement::with_values(
        "INSERT INTO test_bill (id, amount_minor, currency, version, paid) VALUES (?, ?, ?, ?, ?)"
            .to_owned(),
        vec![
            sea_query::Value::from(id),
            sea_query::Value::BigInt(Some(amount)),
            sea_query::Value::from(currency),
            sea_query::Value::BigInt(Some(1)),
            sea_query::Value::BigInt(Some(0)),
        ],
    ))
    .await
    .expect("failed to insert bill");
}

async fn agent_request(
    executor: &Executor<'_, TestAuthenticator, TestPolicy>,
    bill_id: &str,
) -> (String, JournalPreview, BTreeMap<String, String>) {
    let cmd = PayBillCommand;
    let input = PayBillInput {
        bill_id: bill_id.to_string(),
    };
    let ctx = CommandContext {
        company_id: COMPANY_ID.to_string(),
        request_id: "req1".to_string(),
        event_id: "evt1".to_string(),
        now: NOW.to_string(),
    };

    let result = executor
        .run(&cmd, "agent".to_string(), ctx, &input, Execute::Run)
        .await
        .expect("execution should succeed");

    match result {
        Executed::NeedsApproval {
            request_hash,
            preview,
            reason: _,
            reads: _,
        } => (request_hash, preview, BTreeMap::new()),
        _ => panic!("expected NeedsApproval"),
    }
}

async fn bill_paid(db: &dyn Database, bill_id: &str) -> bool {
    let rows = db
        .query(&Statement::with_values(
            "SELECT paid FROM test_bill WHERE id = ?".to_owned(),
            vec![sea_query::Value::from(bill_id)],
        ))
        .await
        .expect("query should succeed");

    rows.first().and_then(|r| r.get::<i64>("paid")) == Some(1)
}

async fn approval_status(db: &dyn Database, approval_id: &str) -> String {
    let rows = db
        .query(&Statement::with_values(
            "SELECT status FROM approval_request WHERE id = ?".to_owned(),
            vec![sea_query::Value::from(approval_id)],
        ))
        .await
        .expect("query should succeed");

    rows.first()
        .and_then(|r| r.get::<String>("status"))
        .expect("approval should exist")
}

// ============================================================================
// Tests
// ============================================================================

#[test]
fn happy_path_approval_flow() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_db(&*kit.db).await;
            insert_bill(&*kit.db, "bill1", 10000, "EUR").await;

            let auth = TestAuthenticator;
            let policy = TestPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let (request_hash, preview, reads) = agent_request(&executor, "bill1").await;

            let approvals = Approvals::new(
                &*kit.db,
                OwnerApprovesAll,
                Es256Verifier,
                TestKeys::new(),
                ApprovalConfig::new(
                    "localhost".to_string(),
                    "http://localhost:3000".to_string(),
                    "/approvals".to_string(),
                ),
            );

            let approval_id = uuid::Uuid::new_v4().to_string();
            approvals
                .raise(
                    "bill.pay",
                    &serde_json::to_value(&PayBillInput {
                        bill_id: "bill1".to_string(),
                    })
                    .unwrap(),
                    &serde_json::to_value(&preview).unwrap(),
                    &reads,
                    request_hash,
                    "payout requires approval".to_string(),
                    AGENT_KEY_ID.to_string(),
                    COMPANY_ID,
                    approval_id.clone(),
                    Some("Test Company"),
                    NOW,
                )
                .await
                .expect("raise should succeed");

            let nonce_bytes = [1u8; 32];
            let challenge = approvals
                .issue_nonce(&approval_id, &nonce_bytes, NOW)
                .await
                .expect("issue_nonce should succeed");

            let virtual_auth = VirtualAuthenticator::new_with_fixed_seed();
            let proof = virtual_auth.assert(&challenge, "localhost", "http://localhost:3000", 0x05);

            let ctx = CommandContext {
                company_id: COMPANY_ID.to_string(),
                request_id: "req1".to_string(),
                event_id: "evt1".to_string(),
                now: NOW.to_string(),
            };

            approvals
                .approve(
                    &executor,
                    &PayBillCommand,
                    &TestAuthenticator,
                    "owner".to_string(),
                    ctx,
                    &approval_id,
                    Some(proof),
                )
                .await
                .expect("approve should succeed");

            assert!(bill_paid(&*kit.db, "bill1").await);
            assert_eq!(approval_status(&*kit.db, &approval_id).await, "approved");
        });
    }
}

#[test]
fn approve_after_bill_changed_is_superseded() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_db(&*kit.db).await;
            insert_bill(&*kit.db, "bill1", 10000, "EUR").await;

            let auth = TestAuthenticator;
            let policy = TestPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let (request_hash, preview, reads) = agent_request(&executor, "bill1").await;

            let approvals = Approvals::new(
                &*kit.db,
                OwnerApprovesAll,
                Es256Verifier,
                TestKeys::new(),
                ApprovalConfig::new(
                    "localhost".to_string(),
                    "http://localhost:3000".to_string(),
                    "/approvals".to_string(),
                ),
            );

            let approval_id = uuid::Uuid::new_v4().to_string();
            approvals
                .raise(
                    "bill.pay",
                    &serde_json::to_value(&PayBillInput {
                        bill_id: "bill1".to_string(),
                    })
                    .unwrap(),
                    &serde_json::to_value(&preview).unwrap(),
                    &reads,
                    request_hash,
                    "payout requires approval".to_string(),
                    AGENT_KEY_ID.to_string(),
                    COMPANY_ID,
                    approval_id.clone(),
                    None,
                    NOW,
                )
                .await
                .expect("raise should succeed");

            let nonce_bytes = [1u8; 32];
            let challenge = approvals
                .issue_nonce(&approval_id, &nonce_bytes, NOW)
                .await
                .expect("issue_nonce should succeed");

            kit.db
                .execute(&Statement::with_values(
                    "UPDATE test_bill SET amount_minor = ?, version = version + 1 WHERE id = ?"
                        .to_owned(),
                    vec![
                        sea_query::Value::BigInt(Some(20000)),
                        sea_query::Value::from("bill1"),
                    ],
                ))
                .await
                .expect("update should succeed");

            let virtual_auth = VirtualAuthenticator::new_with_fixed_seed();
            let proof = virtual_auth.assert(&challenge, "localhost", "http://localhost:3000", 0x05);

            let ctx = CommandContext {
                company_id: COMPANY_ID.to_string(),
                request_id: "req1".to_string(),
                event_id: "evt1".to_string(),
                now: NOW.to_string(),
            };

            let result = approvals
                .approve(
                    &executor,
                    &PayBillCommand,
                    &TestAuthenticator,
                    "owner".to_string(),
                    ctx,
                    &approval_id,
                    Some(proof),
                )
                .await;

            assert!(matches!(
                result,
                Err(ledgers_approvals::ApprovalError::Superseded)
            ));
            assert_eq!(approval_status(&*kit.db, &approval_id).await, "superseded");
            assert!(!bill_paid(&*kit.db, "bill1").await);
        });
    }
}

#[test]
fn agent_cannot_approve() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_db(&*kit.db).await;
            insert_bill(&*kit.db, "bill1", 10000, "EUR").await;

            let auth = TestAuthenticator;
            let policy = TestPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let (request_hash, preview, reads) = agent_request(&executor, "bill1").await;

            let approvals = Approvals::new(
                &*kit.db,
                OwnerApprovesAll,
                Es256Verifier,
                TestKeys::new(),
                ApprovalConfig::new(
                    "localhost".to_string(),
                    "http://localhost:3000".to_string(),
                    "/approvals".to_string(),
                ),
            );

            let approval_id = uuid::Uuid::new_v4().to_string();
            approvals
                .raise(
                    "bill.pay",
                    &serde_json::to_value(&PayBillInput {
                        bill_id: "bill1".to_string(),
                    })
                    .unwrap(),
                    &serde_json::to_value(&preview).unwrap(),
                    &reads,
                    request_hash,
                    "payout requires approval".to_string(),
                    AGENT_KEY_ID.to_string(),
                    COMPANY_ID,
                    approval_id.clone(),
                    None,
                    NOW,
                )
                .await
                .expect("raise should succeed");

            let nonce_bytes = [1u8; 32];
            let challenge = approvals
                .issue_nonce(&approval_id, &nonce_bytes, NOW)
                .await
                .expect("issue_nonce should succeed");

            let virtual_auth = VirtualAuthenticator::new_with_fixed_seed();
            let proof = virtual_auth.assert(&challenge, "localhost", "http://localhost:3000", 0x05);

            let ctx = CommandContext {
                company_id: COMPANY_ID.to_string(),
                request_id: "req1".to_string(),
                event_id: "evt1".to_string(),
                now: NOW.to_string(),
            };

            let result = approvals
                .approve(
                    &executor,
                    &PayBillCommand,
                    &TestAuthenticator,
                    "agent".to_string(),
                    ctx,
                    &approval_id,
                    Some(proof),
                )
                .await;

            assert!(matches!(
                result,
                Err(ledgers_approvals::ApprovalError::Forbidden)
            ));
            assert!(!bill_paid(&*kit.db, "bill1").await);
        });
    }
}

#[test]
fn reject_is_audited() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_db(&*kit.db).await;
            insert_bill(&*kit.db, "bill1", 10000, "EUR").await;

            let auth = TestAuthenticator;
            let policy = TestPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let (request_hash, preview, reads) = agent_request(&executor, "bill1").await;

            let approvals = Approvals::new(
                &*kit.db,
                OwnerApprovesAll,
                Es256Verifier,
                TestKeys::new(),
                ApprovalConfig::new(
                    "localhost".to_string(),
                    "http://localhost:3000".to_string(),
                    "/approvals".to_string(),
                ),
            );

            let approval_id = uuid::Uuid::new_v4().to_string();
            approvals
                .raise(
                    "bill.pay",
                    &serde_json::to_value(&PayBillInput {
                        bill_id: "bill1".to_string(),
                    })
                    .unwrap(),
                    &serde_json::to_value(&preview).unwrap(),
                    &reads,
                    request_hash,
                    "payout requires approval".to_string(),
                    AGENT_KEY_ID.to_string(),
                    COMPANY_ID,
                    approval_id.clone(),
                    None,
                    NOW,
                )
                .await
                .expect("raise should succeed");

            let nonce_bytes = [1u8; 32];
            let challenge = approvals
                .issue_nonce(&approval_id, &nonce_bytes, NOW)
                .await
                .expect("issue_nonce should succeed");

            let virtual_auth = VirtualAuthenticator::new_with_fixed_seed();
            let proof = virtual_auth.assert(&challenge, "localhost", "http://localhost:3000", 0x05);

            let ctx = CommandContext {
                company_id: COMPANY_ID.to_string(),
                request_id: "req1".to_string(),
                event_id: "evt1".to_string(),
                now: NOW.to_string(),
            };

            approvals
                .reject(
                    &executor,
                    &TestAuthenticator,
                    "owner".to_string(),
                    &approval_id,
                    &ctx,
                    Some(proof),
                )
                .await
                .expect("reject should succeed");

            assert_eq!(approval_status(&*kit.db, &approval_id).await, "rejected");
        });
    }
}

#[test]
fn nonce_is_single_use() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_db(&*kit.db).await;
            insert_bill(&*kit.db, "bill1", 10000, "EUR").await;

            let auth = TestAuthenticator;
            let policy = TestPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let (request_hash, preview, reads) = agent_request(&executor, "bill1").await;

            let approvals = Approvals::new(
                &*kit.db,
                OwnerApprovesAll,
                Es256Verifier,
                TestKeys::new(),
                ApprovalConfig::new(
                    "localhost".to_string(),
                    "http://localhost:3000".to_string(),
                    "/approvals".to_string(),
                ),
            );

            let approval_id = uuid::Uuid::new_v4().to_string();
            approvals
                .raise(
                    "bill.pay",
                    &serde_json::to_value(&PayBillInput {
                        bill_id: "bill1".to_string(),
                    })
                    .unwrap(),
                    &serde_json::to_value(&preview).unwrap(),
                    &reads,
                    request_hash,
                    "payout requires approval".to_string(),
                    AGENT_KEY_ID.to_string(),
                    COMPANY_ID,
                    approval_id.clone(),
                    None,
                    NOW,
                )
                .await
                .expect("raise should succeed");

            let nonce_bytes = [1u8; 32];
            let challenge = approvals
                .issue_nonce(&approval_id, &nonce_bytes, NOW)
                .await
                .expect("issue_nonce should succeed");

            let virtual_auth = VirtualAuthenticator::new_with_fixed_seed();
            let proof = virtual_auth.assert(&challenge, "localhost", "http://localhost:3000", 0x05);

            let ctx = CommandContext {
                company_id: COMPANY_ID.to_string(),
                request_id: "req1".to_string(),
                event_id: "evt1".to_string(),
                now: NOW.to_string(),
            };

            approvals
                .approve(
                    &executor,
                    &PayBillCommand,
                    &TestAuthenticator,
                    "owner".to_string(),
                    ctx.clone(),
                    &approval_id,
                    Some(proof.clone()),
                )
                .await
                .expect("first approve should succeed");

            let result = approvals
                .approve(
                    &executor,
                    &PayBillCommand,
                    &TestAuthenticator,
                    "owner".to_string(),
                    ctx,
                    &approval_id,
                    Some(proof),
                )
                .await;

            assert!(
                result.is_err(),
                "second approval with same nonce should fail"
            );
        });
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn assertion_for_a_is_rejected_for_b() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_db(&*kit.db).await;
            insert_bill(&*kit.db, "bill-a", 10000, "EUR").await;
            insert_bill(&*kit.db, "bill-b", 20000, "EUR").await;

            let auth = TestAuthenticator;
            let policy = TestPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let (request_hash_a, preview_a, reads_a) = agent_request(&executor, "bill-a").await;
            let (request_hash_b, preview_b, reads_b) = agent_request(&executor, "bill-b").await;

            let approvals = Approvals::new(
                &*kit.db,
                OwnerApprovesAll,
                Es256Verifier,
                TestKeys::new(),
                ApprovalConfig::new(
                    "localhost".to_string(),
                    "http://localhost:3000".to_string(),
                    "/approvals".to_string(),
                ),
            );

            let approval_id_a = uuid::Uuid::new_v4().to_string();
            approvals
                .raise(
                    "bill.pay",
                    &serde_json::to_value(&PayBillInput {
                        bill_id: "bill-a".to_string(),
                    })
                    .unwrap(),
                    &serde_json::to_value(&preview_a).unwrap(),
                    &reads_a,
                    request_hash_a,
                    "payout A".to_string(),
                    AGENT_KEY_ID.to_string(),
                    COMPANY_ID,
                    approval_id_a.clone(),
                    None,
                    NOW,
                )
                .await
                .expect("raise A should succeed");

            let approval_id_b = uuid::Uuid::new_v4().to_string();
            approvals
                .raise(
                    "bill.pay",
                    &serde_json::to_value(&PayBillInput {
                        bill_id: "bill-b".to_string(),
                    })
                    .unwrap(),
                    &serde_json::to_value(&preview_b).unwrap(),
                    &reads_b,
                    request_hash_b,
                    "payout B".to_string(),
                    AGENT_KEY_ID.to_string(),
                    COMPANY_ID,
                    approval_id_b.clone(),
                    None,
                    NOW,
                )
                .await
                .expect("raise B should succeed");

            let nonce_bytes = [1u8; 32];
            let challenge_a = approvals
                .issue_nonce(&approval_id_a, &nonce_bytes, NOW)
                .await
                .expect("issue_nonce for A should succeed");

            let virtual_auth = VirtualAuthenticator::new_with_fixed_seed();
            let proof_a =
                virtual_auth.assert(&challenge_a, "localhost", "http://localhost:3000", 0x05);

            let ctx = CommandContext {
                company_id: COMPANY_ID.to_string(),
                request_id: "req1".to_string(),
                event_id: "evt1".to_string(),
                now: NOW.to_string(),
            };

            let result = approvals
                .approve(
                    &executor,
                    &PayBillCommand,
                    &TestAuthenticator,
                    "owner".to_string(),
                    ctx.clone(),
                    &approval_id_b,
                    Some(proof_a.clone()),
                )
                .await;

            assert!(result.is_err(), "approving B with A's proof should fail");

            let nonce_bytes_b = [2u8; 32];
            let _challenge_b = approvals
                .issue_nonce(&approval_id_b, &nonce_bytes_b, NOW)
                .await
                .expect("issue_nonce for B should succeed");

            let result = approvals
                .approve(
                    &executor,
                    &PayBillCommand,
                    &TestAuthenticator,
                    "owner".to_string(),
                    ctx.clone(),
                    &approval_id_b,
                    Some(proof_a.clone()),
                )
                .await;

            assert!(result.is_err(), "approving B with A's proof should fail");
            assert_eq!(approval_status(&*kit.db, &approval_id_b).await, "pending");
            assert!(!bill_paid(&*kit.db, "bill-b").await);

            approvals
                .approve(
                    &executor,
                    &PayBillCommand,
                    &TestAuthenticator,
                    "owner".to_string(),
                    ctx,
                    &approval_id_a,
                    Some(proof_a),
                )
                .await
                .expect("approve A should succeed");

            assert!(bill_paid(&*kit.db, "bill-a").await);
        });
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn expired_request_cannot_be_approved() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_db(&*kit.db).await;
            insert_bill(&*kit.db, "bill1", 10000, "EUR").await;
            insert_bill(&*kit.db, "bill2", 15000, "EUR").await;

            let auth = TestAuthenticator;
            let policy = TestPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let (request_hash, preview, reads) = agent_request(&executor, "bill1").await;

            let approvals = Approvals::new(
                &*kit.db,
                OwnerApprovesAll,
                Es256Verifier,
                TestKeys::new(),
                ApprovalConfig::new(
                    "localhost".to_string(),
                    "http://localhost:3000".to_string(),
                    "/approvals".to_string(),
                )
                .with_ttl(std::time::Duration::from_secs(60)),
            );

            let approval_id = uuid::Uuid::new_v4().to_string();
            approvals
                .raise(
                    "bill.pay",
                    &serde_json::to_value(&PayBillInput {
                        bill_id: "bill1".to_string(),
                    })
                    .unwrap(),
                    &serde_json::to_value(&preview).unwrap(),
                    &reads,
                    request_hash.clone(),
                    "payout requires approval".to_string(),
                    AGENT_KEY_ID.to_string(),
                    COMPANY_ID,
                    approval_id.clone(),
                    None,
                    NOW,
                )
                .await
                .expect("raise should succeed");

            let now_110s = "2026-01-15T09:31:50Z";
            let nonce_bytes = [1u8; 32];
            let challenge = approvals
                .issue_nonce(&approval_id, &nonce_bytes, now_110s)
                .await
                .expect("issue_nonce should succeed");

            let now_120s = "2026-01-15T09:32:00Z";
            let virtual_auth = VirtualAuthenticator::new_with_fixed_seed();
            let proof = virtual_auth.assert(&challenge, "localhost", "http://localhost:3000", 0x05);

            let ctx_expired = CommandContext {
                company_id: COMPANY_ID.to_string(),
                request_id: "req2".to_string(),
                event_id: "evt2".to_string(),
                now: now_120s.to_string(),
            };

            let result = approvals
                .approve(
                    &executor,
                    &PayBillCommand,
                    &TestAuthenticator,
                    "owner".to_string(),
                    ctx_expired.clone(),
                    &approval_id,
                    Some(proof),
                )
                .await;

            assert!(matches!(
                result,
                Err(ledgers_approvals::ApprovalError::Expired)
            ));
            assert_eq!(approval_status(&*kit.db, &approval_id).await, "expired");
            assert!(!bill_paid(&*kit.db, "bill1").await);

            let (request_hash2, preview2, reads2) = agent_request(&executor, "bill2").await;
            let approvals2 = Approvals::new(
                &*kit.db,
                OwnerApprovesAll,
                Es256Verifier,
                TestKeys::new(),
                ApprovalConfig::new(
                    "localhost".to_string(),
                    "http://localhost:3000".to_string(),
                    "/approvals".to_string(),
                )
                .with_ttl(std::time::Duration::from_secs(600)),
            );

            let approval_id2 = uuid::Uuid::new_v4().to_string();
            approvals2
                .raise(
                    "bill.pay",
                    &serde_json::to_value(&PayBillInput {
                        bill_id: "bill2".to_string(),
                    })
                    .unwrap(),
                    &serde_json::to_value(&preview2).unwrap(),
                    &reads2,
                    request_hash2,
                    "payout requires approval".to_string(),
                    AGENT_KEY_ID.to_string(),
                    COMPANY_ID,
                    approval_id2.clone(),
                    None,
                    NOW,
                )
                .await
                .expect("raise should succeed");

            let nonce_bytes2 = [2u8; 32];
            let challenge2 = approvals2
                .issue_nonce(&approval_id2, &nonce_bytes2, now_110s)
                .await
                .expect("issue_nonce should succeed");

            let virtual_auth2 = VirtualAuthenticator::new_with_fixed_seed();
            let proof2 =
                virtual_auth2.assert(&challenge2, "localhost", "http://localhost:3000", 0x05);

            approvals2
                .approve(
                    &executor,
                    &PayBillCommand,
                    &TestAuthenticator,
                    "owner".to_string(),
                    ctx_expired,
                    &approval_id2,
                    Some(proof2),
                )
                .await
                .expect("approve should succeed with longer ttl");

            assert!(bill_paid(&*kit.db, "bill2").await);
        });
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn superseded_assertion_cannot_approve_twin() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_db(&*kit.db).await;
            insert_bill(&*kit.db, "bill1", 10000, "EUR").await;

            let auth = TestAuthenticator;
            let policy = TestPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let (request_hash, preview, reads) = agent_request(&executor, "bill1").await;

            let approvals = Approvals::new(
                &*kit.db,
                OwnerApprovesAll,
                Es256Verifier,
                TestKeys::new(),
                ApprovalConfig::new(
                    "localhost".to_string(),
                    "http://localhost:3000".to_string(),
                    "/approvals".to_string(),
                ),
            );

            let approval_id_a = uuid::Uuid::new_v4().to_string();
            approvals
                .raise(
                    "bill.pay",
                    &serde_json::to_value(&PayBillInput {
                        bill_id: "bill1".to_string(),
                    })
                    .unwrap(),
                    &serde_json::to_value(&preview).unwrap(),
                    &reads,
                    request_hash.clone(),
                    "payout requires approval".to_string(),
                    AGENT_KEY_ID.to_string(),
                    COMPANY_ID,
                    approval_id_a.clone(),
                    None,
                    NOW,
                )
                .await
                .expect("raise should succeed");

            let nonce_bytes_a = [1u8; 32];
            let challenge_a = approvals
                .issue_nonce(&approval_id_a, &nonce_bytes_a, NOW)
                .await
                .expect("issue_nonce should succeed");

            let virtual_auth = VirtualAuthenticator::new_with_fixed_seed();
            let proof_a =
                virtual_auth.assert(&challenge_a, "localhost", "http://localhost:3000", 0x05);

            kit.db
                .execute(&Statement::with_values(
                    "UPDATE test_bill SET amount_minor = ?, version = version + 1 WHERE id = ?"
                        .to_owned(),
                    vec![
                        sea_query::Value::BigInt(Some(20000)),
                        sea_query::Value::from("bill1"),
                    ],
                ))
                .await
                .expect("update should succeed");

            let ctx = CommandContext {
                company_id: COMPANY_ID.to_string(),
                request_id: "req1".to_string(),
                event_id: "evt1".to_string(),
                now: NOW.to_string(),
            };

            let result = approvals
                .approve(
                    &executor,
                    &PayBillCommand,
                    &TestAuthenticator,
                    "owner".to_string(),
                    ctx.clone(),
                    &approval_id_a,
                    Some(proof_a.clone()),
                )
                .await;

            assert!(matches!(
                result,
                Err(ledgers_approvals::ApprovalError::Superseded)
            ));

            let (request_hash_b, preview_b, reads_b) = agent_request(&executor, "bill1").await;
            let approval_id_b = uuid::Uuid::new_v4().to_string();
            approvals
                .raise(
                    "bill.pay",
                    &serde_json::to_value(&PayBillInput {
                        bill_id: "bill1".to_string(),
                    })
                    .unwrap(),
                    &serde_json::to_value(&preview_b).unwrap(),
                    &reads_b,
                    request_hash_b,
                    "payout requires approval".to_string(),
                    AGENT_KEY_ID.to_string(),
                    COMPANY_ID,
                    approval_id_b.clone(),
                    None,
                    NOW,
                )
                .await
                .expect("raise B should succeed");

            let result = approvals
                .approve(
                    &executor,
                    &PayBillCommand,
                    &TestAuthenticator,
                    "owner".to_string(),
                    ctx.clone(),
                    &approval_id_b,
                    Some(proof_a),
                )
                .await;

            assert!(result.is_err(), "approving B with A's proof should fail");

            let nonce_bytes_b = [2u8; 32];
            let challenge_b = approvals
                .issue_nonce(&approval_id_b, &nonce_bytes_b, NOW)
                .await
                .expect("issue_nonce for B should succeed");

            let proof_b =
                virtual_auth.assert(&challenge_b, "localhost", "http://localhost:3000", 0x05);

            approvals
                .approve(
                    &executor,
                    &PayBillCommand,
                    &TestAuthenticator,
                    "owner".to_string(),
                    ctx,
                    &approval_id_b,
                    Some(proof_b),
                )
                .await
                .expect("approve B should succeed");

            assert!(bill_paid(&*kit.db, "bill1").await);
        });
    }
}

#[test]
fn high_risk_without_passkey_is_refused() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_db(&*kit.db).await;
            insert_bill(&*kit.db, "bill1", 10000, "EUR").await;

            let auth = TestAuthenticator;
            let policy = TestPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let (request_hash, preview, reads) = agent_request(&executor, "bill1").await;

            let approvals = Approvals::new(
                &*kit.db,
                OwnerApprovesAll,
                Es256Verifier,
                TestKeys::new(),
                ApprovalConfig::new(
                    "localhost".to_string(),
                    "http://localhost:3000".to_string(),
                    "/approvals".to_string(),
                ),
            );

            let approval_id = uuid::Uuid::new_v4().to_string();
            approvals
                .raise(
                    "bill.pay",
                    &serde_json::to_value(&PayBillInput {
                        bill_id: "bill1".to_string(),
                    })
                    .unwrap(),
                    &serde_json::to_value(&preview).unwrap(),
                    &reads,
                    request_hash,
                    "payout requires approval".to_string(),
                    AGENT_KEY_ID.to_string(),
                    COMPANY_ID,
                    approval_id.clone(),
                    None,
                    NOW,
                )
                .await
                .expect("raise should succeed");

            let ctx = CommandContext {
                company_id: COMPANY_ID.to_string(),
                request_id: "req1".to_string(),
                event_id: "evt1".to_string(),
                now: NOW.to_string(),
            };

            // Try to approve without passkey proof (proof=None)
            let result = approvals
                .approve(
                    &executor,
                    &PayBillCommand,
                    &TestAuthenticator,
                    "owner".to_string(),
                    ctx,
                    &approval_id,
                    None, // No proof for high-risk payment
                )
                .await;

            // Should fail with StepUpRequired
            assert!(matches!(
                result,
                Err(ledgers_approvals::ApprovalError::StepUpRequired)
            ));

            // Verify status is still pending
            assert_eq!(approval_status(&*kit.db, &approval_id).await, "pending");
        });
    }
}

// ============================================================================
// Inbox: needs-approval persistence, decisions, expiry, fresh sign-in, bulk
// ============================================================================

/// `NOW` as epoch seconds (2026-01-15T09:30:00Z).
const NOW_EPOCH: i64 = 1_768_469_400;
/// The low-risk limit used by the inbox tests: EUR 100.00.
fn eur_limit() -> BTreeMap<String, String> {
    BTreeMap::from([("EUR".to_string(), "100.00".to_string())])
}

fn inbox_ctx(now: &str, event: &str) -> CommandContext {
    CommandContext {
        company_id: COMPANY_ID.to_string(),
        request_id: "req-inbox".to_string(),
        event_id: event.to_string(),
        now: now.to_string(),
    }
}

/// The agent asks to pay a bill; policy sends it to approval. The request is
/// stored by the executor in the same guarded batch as its audit row.
async fn agent_asks_to_pay(
    db: &dyn Database,
    approval_id: &str,
    bill_id: &str,
    limits: &BTreeMap<String, String>,
) -> Executed<PayBillOutput> {
    let executor = Executor::new(db, TestAuthenticator, TestPolicy);
    let input = PayBillInput {
        bill_id: bill_id.to_string(),
    };
    let id = approval_id.to_string();
    let persist = move |pending: &PendingApproval| {
        let preview = serde_json::to_value(&pending.preview)
            .map_err(|e| ledgers_commands::CommandError::InvalidInput(e.to_string()))?;
        let (statement, _) = ledgers_approvals::request_statement(&ledgers_approvals::NewRequest {
            id: &id,
            company_id: COMPANY_ID,
            command: &pending.command,
            input: &pending.input,
            preview: &preview,
            reads: &pending.reads,
            request_hash: &pending.request_hash,
            reason: &pending.reason,
            requested_by: &pending.requested_by,
            counterparty: Some("Acme GmbH"),
            now: NOW,
            ttl: std::time::Duration::from_hours(24),
            low_risk_limits: limits,
        })
        .map_err(|e| ledgers_commands::CommandError::InvalidInput(e.to_string()))?;
        Ok(vec![statement])
    };
    executor
        .run_for_approval(
            &PayBillCommand,
            "agent".to_string(),
            inbox_ctx(NOW, "evt-agent"),
            &input,
            &persist,
        )
        .await
        .expect("the agent call should return a needs-approval outcome")
}

fn inbox_service(
    db: &dyn Database,
) -> Approvals<'_, OwnerApprovesAll, Es256Verifier, NoCredentialKeys> {
    Approvals::new(
        db,
        OwnerApprovesAll,
        Es256Verifier,
        NoCredentialKeys,
        ApprovalConfig::new(
            "localhost".to_string(),
            "http://localhost:3000".to_string(),
            "http://localhost:3000".to_string(),
        ),
    )
}

async fn count_rows(db: &dyn Database, sql: &str, id: &str) -> i64 {
    db.query(&Statement::with_values(
        sql.to_owned(),
        vec![sea_query::Value::from(id)],
    ))
    .await
    .expect("count query")
    .first()
    .and_then(|r| r.get::<i64>("n"))
    .unwrap_or(0)
}

#[test]
fn needs_approval_persists_exactly_one_request() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_db(&*kit.db).await;
            insert_bill(&*kit.db, "bill1", 10000, "EUR").await;

            let outcome = agent_asks_to_pay(&*kit.db, "appr-1", "bill1", &eur_limit()).await;
            assert!(matches!(outcome, Executed::NeedsApproval { .. }));

            // Exactly one request, with the agent as requester and a risk and expiry.
            let total = count_rows(
                &*kit.db,
                "SELECT COUNT(*) AS n FROM approval_request WHERE id = ?",
                "appr-1",
            )
            .await;
            assert_eq!(total, 1);
            let request = get_request(&*kit.db, COMPANY_ID, "appr-1")
                .await
                .expect("the request is stored");
            assert_eq!(request.requested_by, AGENT_KEY_ID);
            assert_eq!(request.risk, ledgers_approvals::Risk::Low);
            assert_eq!(request.counterparty.as_deref(), Some("Acme GmbH"));
            assert_eq!(request.expires_at, "2026-01-16T09:30:00Z");
            assert_eq!(request.status, ledgers_approvals::Status::Pending);
        });
    }
}

#[test]
fn approve_posts_once_even_when_called_twice_concurrently() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_db(&*kit.db).await;
            insert_bill(&*kit.db, "bill1", 10000, "EUR").await;
            agent_asks_to_pay(&*kit.db, "appr-2", "bill1", &eur_limit()).await;

            let executor = Executor::new(&*kit.db, TestAuthenticator, TestPolicy);
            let approvals = inbox_service(&*kit.db);
            let input = PayBillInput {
                bill_id: "bill1".to_string(),
            };
            let first = approvals.approve_signed_in(
                &executor,
                &TestAuthenticator,
                &PayBillCommand,
                "owner".to_string(),
                inbox_ctx(NOW, "evt-a"),
                "appr-2",
                Some(NOW_EPOCH),
            );
            let second = approvals.approve_signed_in(
                &executor,
                &TestAuthenticator,
                &PayBillCommand,
                "owner".to_string(),
                inbox_ctx(NOW, "evt-b"),
                "appr-2",
                Some(NOW_EPOCH),
            );
            let (a, b) = futures_util::future::join(first, second).await;
            let _ = &input;
            let outcomes = [a.is_ok(), b.is_ok()];
            assert_eq!(
                outcomes.iter().filter(|ok| **ok).count(),
                1,
                "exactly one approval posts"
            );
            let loser = if a.is_ok() { b } else { a };
            assert!(matches!(loser, Err(ApprovalError::NotPending { .. })));
            assert!(bill_paid(&*kit.db, "bill1").await);
            assert_eq!(approval_status(&*kit.db, "appr-2").await, "approved");
        });
    }
}

#[test]
fn reject_records_the_reason_and_posts_nothing() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_db(&*kit.db).await;
            insert_bill(&*kit.db, "bill1", 10000, "EUR").await;
            agent_asks_to_pay(&*kit.db, "appr-3", "bill1", &eur_limit()).await;

            let executor = Executor::new(&*kit.db, TestAuthenticator, TestPolicy);
            inbox_service(&*kit.db)
                .reject_signed_in(
                    &executor,
                    &TestAuthenticator,
                    "owner".to_string(),
                    "appr-3",
                    &inbox_ctx(NOW, "evt-r"),
                    Some("amount is wrong"),
                )
                .await
                .expect("reject should succeed");

            assert_eq!(approval_status(&*kit.db, "appr-3").await, "rejected");
            assert!(!bill_paid(&*kit.db, "bill1").await);
            let request = get_request(&*kit.db, COMPANY_ID, "appr-3").await.unwrap();
            assert_eq!(request.decision_reason.as_deref(), Some("amount is wrong"));
            assert_eq!(request.audit_ref.as_deref(), Some("evt-r"));
            assert_eq!(request.decided_by.as_deref(), Some(OWNER_PERSON_ID));
        });
    }
}

/// The outbox notices for one approval request, as (outbox id, parsed payload).
async fn notices_for(db: &dyn Database, approval_id: &str) -> Vec<(String, serde_json::Value)> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT id, payload FROM approval_outbox WHERE id LIKE ? ORDER BY id".to_owned(),
            vec![sea_query::Value::from(format!("{approval_id}-%"))],
        ))
        .await
        .expect("query the notice outbox");
    rows.rows
        .iter()
        .map(|row| {
            let id: String = row.get("id").expect("outbox id");
            let payload: String = row.get("payload").expect("outbox payload");
            (
                id,
                serde_json::from_str(&payload).expect("notice payload is JSON"),
            )
        })
        .collect()
}

#[test]
fn raise_enqueues_a_created_notice_linking_the_app() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_db(&*kit.db).await;
            insert_bill(&*kit.db, "bill1", 10000, "EUR").await;
            let executor = Executor::new(&*kit.db, TestAuthenticator, TestPolicy);
            let (request_hash, preview, reads) = agent_request(&executor, "bill1").await;

            let approval_id = "appr-raise-notice";
            inbox_service(&*kit.db)
                .raise(
                    "bill.pay",
                    &serde_json::to_value(&PayBillInput {
                        bill_id: "bill1".to_string(),
                    })
                    .unwrap(),
                    &serde_json::to_value(&preview).unwrap(),
                    &reads,
                    request_hash,
                    "payout requires approval".to_string(),
                    AGENT_KEY_ID.to_string(),
                    COMPANY_ID,
                    approval_id.to_string(),
                    Some("Acme GmbH"),
                    NOW,
                )
                .await
                .expect("raise should succeed");

            let notices = notices_for(&*kit.db, approval_id).await;
            assert_eq!(notices.len(), 1, "one created notice");
            let (id, payload) = &notices[0];
            assert_eq!(id, &format!("{approval_id}-created"));
            assert_eq!(payload["event"], "created");
            assert_eq!(payload["status"], "pending");
            assert_eq!(payload["counterparty"], "Acme GmbH");
            assert_eq!(
                payload["link"],
                format!("http://localhost:3000/approvals/{approval_id}")
            );
        });
    }
}

#[test]
fn inbox_decisions_enqueue_a_decided_notice_and_refusals_enqueue_none() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_db(&*kit.db).await;
            insert_bill(&*kit.db, "bill1", 10000, "EUR").await;

            agent_asks_to_pay(&*kit.db, "appr-n-ok", "bill1", &eur_limit()).await;
            let executor = Executor::new(&*kit.db, TestAuthenticator, TestPolicy);
            inbox_service(&*kit.db)
                .approve_signed_in(
                    &executor,
                    &TestAuthenticator,
                    &PayBillCommand,
                    "owner".to_string(),
                    inbox_ctx(NOW, "evt-n-ok"),
                    "appr-n-ok",
                    Some(NOW_EPOCH),
                )
                .await
                .expect("approve should succeed");
            // The test persist step stores the request without a Created notice
            // (the server dispatch and `raise` enqueue that), so only Decided shows.
            let approved = notices_for(&*kit.db, "appr-n-ok").await;
            assert_eq!(approved.len(), 1, "one decided notice");
            let (id, payload) = &approved[0];
            assert_eq!(id, "appr-n-ok-decided");
            assert_eq!(payload["status"], "approved");
            assert_eq!(payload["link"], "http://localhost:3000/approvals/appr-n-ok");

            agent_asks_to_pay(&*kit.db, "appr-n-no", "bill1", &eur_limit()).await;
            inbox_service(&*kit.db)
                .reject_signed_in(
                    &executor,
                    &TestAuthenticator,
                    "owner".to_string(),
                    "appr-n-no",
                    &inbox_ctx(NOW, "evt-n-no"),
                    Some("amount is wrong"),
                )
                .await
                .expect("reject should succeed");
            let rejected = notices_for(&*kit.db, "appr-n-no").await;
            assert_eq!(rejected.len(), 1, "one decided notice");
            assert_eq!(rejected[0].1["status"], "rejected");

            // A refused decision (expired) posts nothing and enqueues no decided notice.
            agent_asks_to_pay(&*kit.db, "appr-n-exp", "bill1", &eur_limit()).await;
            let refused = inbox_service(&*kit.db)
                .approve_signed_in(
                    &executor,
                    &TestAuthenticator,
                    &PayBillCommand,
                    "owner".to_string(),
                    inbox_ctx("2026-01-17T09:30:00Z", "evt-n-exp"),
                    "appr-n-exp",
                    Some(NOW_EPOCH),
                )
                .await;
            assert!(matches!(refused, Err(ApprovalError::Expired)));
            let expired = notices_for(&*kit.db, "appr-n-exp").await;
            assert!(expired.is_empty(), "a refused decision enqueues nothing");
        });
    }
}

#[test]
fn expired_request_cannot_be_approved_from_the_inbox() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_db(&*kit.db).await;
            insert_bill(&*kit.db, "bill1", 10000, "EUR").await;
            agent_asks_to_pay(&*kit.db, "appr-4", "bill1", &eur_limit()).await;

            let executor = Executor::new(&*kit.db, TestAuthenticator, TestPolicy);
            let result = inbox_service(&*kit.db)
                .approve_signed_in(
                    &executor,
                    &TestAuthenticator,
                    &PayBillCommand,
                    "owner".to_string(),
                    inbox_ctx("2026-01-17T09:30:00Z", "evt-x"),
                    "appr-4",
                    Some(NOW_EPOCH),
                )
                .await;
            assert!(matches!(result, Err(ApprovalError::Expired)));
            assert!(!bill_paid(&*kit.db, "bill1").await);
            assert_eq!(approval_status(&*kit.db, "appr-4").await, "expired");
        });
    }
}

#[test]
fn agent_cannot_decide_from_the_inbox() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_db(&*kit.db).await;
            insert_bill(&*kit.db, "bill1", 10000, "EUR").await;
            agent_asks_to_pay(&*kit.db, "appr-5", "bill1", &eur_limit()).await;

            let executor = Executor::new(&*kit.db, TestAuthenticator, TestPolicy);
            let approve = inbox_service(&*kit.db)
                .approve_signed_in(
                    &executor,
                    &TestAuthenticator,
                    &PayBillCommand,
                    "agent".to_string(),
                    inbox_ctx(NOW, "evt-y"),
                    "appr-5",
                    Some(NOW_EPOCH),
                )
                .await;
            assert!(matches!(approve, Err(ApprovalError::Forbidden)));
            let reject = inbox_service(&*kit.db)
                .reject_signed_in(
                    &executor,
                    &TestAuthenticator,
                    "agent".to_string(),
                    "appr-5",
                    &inbox_ctx(NOW, "evt-z"),
                    None,
                )
                .await;
            assert!(matches!(reject, Err(ApprovalError::Forbidden)));
            assert!(!bill_paid(&*kit.db, "bill1").await);
            assert_eq!(approval_status(&*kit.db, "appr-5").await, "pending");
        });
    }
}

#[test]
fn high_risk_approval_needs_a_fresh_sign_in() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_db(&*kit.db).await;
            insert_bill(&*kit.db, "bill1", 10000, "EUR").await;
            // No limit configured for EUR, so the request is high risk.
            agent_asks_to_pay(&*kit.db, "appr-6", "bill1", &BTreeMap::new()).await;
            assert_eq!(
                get_request(&*kit.db, COMPANY_ID, "appr-6")
                    .await
                    .unwrap()
                    .risk,
                ledgers_approvals::Risk::High
            );

            let executor = Executor::new(&*kit.db, TestAuthenticator, TestPolicy);
            let approvals = inbox_service(&*kit.db);
            let approve = |signed_in_at: Option<i64>, event: &'static str| {
                approvals.approve_signed_in(
                    &executor,
                    &TestAuthenticator,
                    &PayBillCommand,
                    "owner".to_string(),
                    inbox_ctx(NOW, event),
                    "appr-6",
                    signed_in_at,
                )
            };
            // No sign-in, or one older than five minutes: reauth required, nothing posted.
            assert!(matches!(
                approve(None, "e1").await,
                Err(ApprovalError::ReauthRequired)
            ));
            assert!(matches!(
                approve(Some(NOW_EPOCH - 301), "e2").await,
                Err(ApprovalError::ReauthRequired)
            ));
            assert!(!bill_paid(&*kit.db, "bill1").await);
            assert_eq!(approval_status(&*kit.db, "appr-6").await, "pending");
            // Within five minutes: approved.
            approve(Some(NOW_EPOCH - 60), "e3")
                .await
                .expect("fresh sign-in approves");
            assert!(bill_paid(&*kit.db, "bill1").await);
        });
    }
}

#[test]
fn low_risk_approval_needs_no_fresh_sign_in() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_db(&*kit.db).await;
            insert_bill(&*kit.db, "bill1", 10000, "EUR").await;
            agent_asks_to_pay(&*kit.db, "appr-7", "bill1", &eur_limit()).await;
            let executor = Executor::new(&*kit.db, TestAuthenticator, TestPolicy);
            inbox_service(&*kit.db)
                .approve_signed_in(
                    &executor,
                    &TestAuthenticator,
                    &PayBillCommand,
                    "owner".to_string(),
                    inbox_ctx(NOW, "e4"),
                    "appr-7",
                    None,
                )
                .await
                .expect("low risk approves without a recent sign-in");
            assert!(bill_paid(&*kit.db, "bill1").await);
        });
    }
}

#[test]
fn bulk_selection_refuses_mixed_risk_and_mixed_currency() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_db(&*kit.db).await;
            insert_bill(&*kit.db, "bill1", 10000, "EUR").await;
            insert_bill(&*kit.db, "bill2", 5000, "EUR").await;
            insert_bill(&*kit.db, "bill3", 8000, "EUR").await;
            insert_bill(&*kit.db, "bill4", 9000, "USD").await;
            let limits = BTreeMap::from([
                ("EUR".to_string(), "100.00".to_string()),
                ("USD".to_string(), "100.00".to_string()),
            ]);
            agent_asks_to_pay(&*kit.db, "b-1", "bill1", &limits).await;
            agent_asks_to_pay(&*kit.db, "b-2", "bill2", &limits).await;
            agent_asks_to_pay(&*kit.db, "b-3", "bill3", &BTreeMap::new()).await;
            agent_asks_to_pay(&*kit.db, "b-4", "bill4", &limits).await;

            let load = |id: &'static str| {
                let db = &*kit.db;
                async move { get_request(db, COMPANY_ID, id).await.unwrap() }
            };
            let low_eur_a = load("b-1").await;
            let low_eur_b = load("b-2").await;
            let high_eur = load("b-3").await;
            let low_usd = load("b-4").await;

            assert!(ledgers_approvals::check_bulk(&[low_eur_a.clone(), low_eur_b.clone()]).is_ok());
            assert!(matches!(
                ledgers_approvals::check_bulk(&[low_eur_a.clone(), high_eur]),
                Err(ApprovalError::BulkRefused(_))
            ));
            assert!(matches!(
                ledgers_approvals::check_bulk(&[low_eur_a, low_usd]),
                Err(ApprovalError::BulkRefused(_))
            ));
            assert!(matches!(
                ledgers_approvals::check_bulk(&[]),
                Err(ApprovalError::BulkRefused(_))
            ));

            let pending = list_requests(
                &*kit.db,
                COMPANY_ID,
                ledgers_approvals::Listing::Pending,
                NOW,
                50,
            )
            .await
            .expect("list");
            assert_eq!(pending.1, 4);
            assert_eq!(pending.0.len(), 4);
        });
    }
}

/// Counts every statement that reaches the wrapped database.
struct CountingDb<'a> {
    inner: &'a dyn Database,
    statements: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl Database for CountingDb<'_> {
    async fn execute(&self, stmt: &Statement) -> Result<u64, cratefield_core::DbError> {
        self.statements
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.execute(stmt).await
    }

    async fn query(
        &self,
        stmt: &Statement,
    ) -> Result<cratefield_core::Rows, cratefield_core::DbError> {
        self.statements
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.query(stmt).await
    }

    async fn batch_atomic(&self, stmts: &[Statement]) -> Result<(), cratefield_core::DbError> {
        self.statements
            .fetch_add(stmts.len().max(1), std::sync::atomic::Ordering::SeqCst);
        self.inner.batch_atomic(stmts).await
    }
}

/// The approvals inbox lists a page in a constant number of statements: the
/// page's requests come from one select, not one lookup per request. Counts
/// the statements for 2 and for 8 pending requests on every dialect.
#[test]
fn list_requests_is_a_constant_number_of_statements() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_db(&*kit.db).await;
            let limits = BTreeMap::new();
            let mut counts = Vec::new();
            for (round, total) in [(0usize, 2usize), (2, 8)] {
                for n in round..total {
                    insert_bill(&*kit.db, &format!("lb{n}"), 10000, "EUR").await;
                    agent_asks_to_pay(&*kit.db, &format!("la-{n}"), &format!("lb{n}"), &limits)
                        .await;
                }
                let counting = CountingDb {
                    inner: &*kit.db,
                    statements: std::sync::atomic::AtomicUsize::new(0),
                };
                let (requests, pending) = list_requests(
                    &counting,
                    COMPANY_ID,
                    ledgers_approvals::Listing::Pending,
                    NOW,
                    50,
                )
                .await
                .expect("list");
                assert_eq!(requests.len(), total, "every pending request is listed");
                assert_eq!(pending, i64::try_from(total).expect("a small count"));
                for request in &requests {
                    let single = get_request(&*kit.db, COMPANY_ID, &request.id)
                        .await
                        .expect("the single read works");
                    assert_eq!(format!("{request:?}"), format!("{single:?}"));
                }
                counts.push(
                    counting
                        .statements
                        .load(std::sync::atomic::Ordering::SeqCst),
                );
            }
            assert_eq!(
                counts[0], counts[1],
                "listing 8 requests took {} statements, listing 2 took {}: the count must not grow",
                counts[1], counts[0]
            );
        });
    }
}
