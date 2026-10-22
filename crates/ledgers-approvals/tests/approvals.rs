//! Integration tests for the approvals service with passkey step-up.

use cratefield_core::{Database, Statement};
use cratefield_testing::TestHarness;
use ledgers_approvals::{
    ApprovalConfig, Approvals, CredentialKeys, Es256Verifier, OwnerApprovesAll, PasskeyProof,
};
use ledgers_commands::{
    Authenticator, Command, CommandContext, Credential, Execute, Executed, Executor,
    JournalPreview, Plan, Policy, Principal, Tier, auth::Limits,
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
