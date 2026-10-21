//! Integration tests for the command executor against a real database.
#![allow(
    clippy::manual_let_else,
    clippy::must_use_candidate,
    clippy::cast_lossless,
    clippy::unreadable_literal,
    clippy::suspicious_else_formatting
)]

use cratefield_core::{Database, Statement};
use cratefield_testing::TestHarness;
use ledgers_commands::command::Plan;
use ledgers_commands::*;
use ledgers_db::DbModule;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const COMPANY_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const NOW: &str = "2026-01-15T09:30:00Z";

// ============================================================================
// Test Command: a simple write-to-table command with configurable preview
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
struct TestInput {
    message: String,
    #[serde(default)]
    base_amount: Option<i64>,
    #[serde(default)]
    transaction_amount: Option<i64>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct TestOutput {
    row_id: u64,
    message: String,
}

struct TestWriteCommand;

#[async_trait::async_trait(?Send)]
impl Command for TestWriteCommand {
    const NAME: &'static str = "test.write";
    const TIER: Tier = Tier::Draft;
    const HUMAN_ONLY: bool = false;
    const SUPPORTS_DRY_RUN: bool = true;
    const IDEMPOTENT: bool = false;

    type Input = TestInput;
    type Output = TestOutput;

    async fn preview(
        &self,
        _db: &dyn Database,
        _ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        let mut preview = JournalPreview::empty();

        if let Some(base_amt) = input.base_amount {
            let eur = ledgers_money::Currency::from_code("EUR")
                .map_err(|_| CommandError::InvalidInput("EUR not found".to_string()))?;
            let amt = ledgers_money::Amount::new(base_amt as i128, eur);
            preview.add_line("1000".to_string(), "debit", amt, None)?;
        }

        if let Some(txn_amt) = input.transaction_amount {
            let usd = ledgers_money::Currency::from_code("USD")
                .map_err(|_| CommandError::InvalidInput("USD not found".to_string()))?;
            let amt = ledgers_money::Amount::new(txn_amt as i128, usd);
            if let Some(base_amt) = input.base_amount {
                // Add as transaction currency on same line
                let eur = ledgers_money::Currency::from_code("EUR")
                    .map_err(|_| CommandError::InvalidInput("EUR not found".to_string()))?;
                let base = ledgers_money::Amount::new(base_amt as i128, eur);
                let usd = ledgers_money::Currency::from_code("USD")
                    .map_err(|_| CommandError::InvalidInput("USD not found".to_string()))?;
                let txn = ledgers_money::Amount::new(txn_amt as i128, usd);
                preview.add_line("1000".to_string(), "debit", base, Some(txn))?;
            } else {
                preview.add_line("1000".to_string(), "debit", amt, None)?;
            }
        }

        Ok(preview)
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        _db: &dyn Database,
        _ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        // Insert a row into test_write table (simple test statement)
        let stmt = Statement::new("INSERT INTO test_write (message) VALUES ('test_message')");

        Ok(Plan {
            statements: vec![stmt],
            output: TestOutput {
                row_id: 1,
                message: input.message.clone(),
            },
            objects: vec!["test_write:1".to_string()],
        })
    }
}

struct TestHumanOnlyCommand;

#[async_trait::async_trait(?Send)]
impl Command for TestHumanOnlyCommand {
    const NAME: &'static str = "test.human_only";
    const TIER: Tier = Tier::Read;
    const HUMAN_ONLY: bool = true;
    const SUPPORTS_DRY_RUN: bool = false;

    type Input = TestInput;
    type Output = TestOutput;

    async fn preview(
        &self,
        _db: &dyn Database,
        _ctx: &CommandContext,
        _input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        _db: &dyn Database,
        _ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        Ok(Plan {
            statements: vec![],
            output: TestOutput {
                row_id: 1,
                message: input.message.clone(),
            },
            objects: vec![],
        })
    }
}

struct TestReadCommand;

#[async_trait::async_trait(?Send)]
impl Command for TestReadCommand {
    const NAME: &'static str = "test.read";
    const TIER: Tier = Tier::Read;
    const HUMAN_ONLY: bool = false;
    const SUPPORTS_DRY_RUN: bool = false;

    type Input = TestInput;
    type Output = TestOutput;

    async fn preview(
        &self,
        _db: &dyn Database,
        _ctx: &CommandContext,
        _input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        Ok(JournalPreview::empty())
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        _db: &dyn Database,
        _ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        Ok(Plan {
            statements: vec![],
            output: TestOutput {
                row_id: 1,
                message: input.message.clone(),
            },
            objects: vec![],
        })
    }
}

struct TestPostCommand;

#[async_trait::async_trait(?Send)]
impl Command for TestPostCommand {
    const NAME: &'static str = "test.post";
    const TIER: Tier = Tier::PostWithinLimits;
    const HUMAN_ONLY: bool = false;
    const SUPPORTS_DRY_RUN: bool = true;

    type Input = TestInput;
    type Output = TestOutput;

    async fn preview(
        &self,
        _db: &dyn Database,
        _ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<JournalPreview, CommandError> {
        let mut preview = JournalPreview::empty();

        if let Some(base_amt) = input.base_amount {
            let eur = ledgers_money::Currency::from_code("EUR")
                .map_err(|_| CommandError::InvalidInput("EUR not found".to_string()))?;
            let amt = ledgers_money::Amount::new(base_amt as i128, eur);
            preview.add_line("1000".to_string(), "debit", amt, None)?;
        }

        Ok(preview)
    }

    async fn plan(
        &self,
        _token: &ExecToken,
        _db: &dyn Database,
        _ctx: &CommandContext,
        input: &Self::Input,
    ) -> Result<Plan<Self::Output>, CommandError> {
        let stmt = Statement::new("INSERT INTO test_write (message) VALUES ('test_message')");

        Ok(Plan {
            statements: vec![stmt],
            output: TestOutput {
                row_id: 1,
                message: input.message.clone(),
            },
            objects: vec![],
        })
    }
}

// ============================================================================
// Test Authenticator
// ============================================================================

struct TestAuthenticator {
    credentials: BTreeMap<String, Principal>,
}

impl TestAuthenticator {
    fn new() -> Self {
        let mut credentials = BTreeMap::new();

        // Owner
        credentials.insert(
            "bearer-owner".to_string(),
            Principal::Person {
                person_id: "owner-1".to_string(),
            },
        );

        // Read agent
        credentials.insert(
            "bearer-read-agent".to_string(),
            Principal::Agent {
                key_id: "key-read".to_string(),
                tier: Tier::Read,
                limits: Limits::new(),
            },
        );

        // Draft agent
        credentials.insert(
            "bearer-draft-agent".to_string(),
            Principal::Agent {
                key_id: "key-draft".to_string(),
                tier: Tier::Draft,
                limits: Limits::new(),
            },
        );

        // Post agent with EUR limits
        let limits = Limits::new()
            .insert(
                "EUR",
                Limit {
                    base_ceiling: 100000,       // EUR 1000.00
                    transaction_ceiling: 50000, // EUR 500.00
                },
            )
            .expect("valid currency");
        credentials.insert(
            "bearer-post-agent".to_string(),
            Principal::Agent {
                key_id: "key-post".to_string(),
                tier: Tier::PostWithinLimits,
                limits,
            },
        );

        // Post agent with no limits (missing EUR = 0)
        credentials.insert(
            "bearer-post-no-limits".to_string(),
            Principal::Agent {
                key_id: "key-post-no-limits".to_string(),
                tier: Tier::PostWithinLimits,
                limits: Limits::new(),
            },
        );

        TestAuthenticator { credentials }
    }
}

#[async_trait::async_trait(?Send)]
impl Authenticator for TestAuthenticator {
    async fn authenticate(&self, credential: &Credential) -> Result<Principal, CommandError> {
        self.credentials
            .get(credential)
            .cloned()
            .ok_or(CommandError::Unauthenticated)
    }
}

// ============================================================================
// Utilities for database setup
// ============================================================================

async fn setup_test_table(db: &dyn Database) {
    // Create a simple test table for the executor to write to.
    // The executor uses batch_atomic, so we just need a table to exist.
    let _ = db
        .execute(&Statement::new(
            "CREATE TABLE IF NOT EXISTS test_write (id INTEGER PRIMARY KEY, message TEXT NOT NULL)",
        ))
        .await;
}

// ============================================================================
// Integration Tests
// ============================================================================

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

#[test]
fn owner_executes_command_successfully() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_test_table(&*kit.db).await;

            let auth = TestAuthenticator::new();
            let policy = DefaultPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let cmd = TestWriteCommand;
            let input = TestInput {
                message: "owner test".to_string(),
                base_amount: None,
                transaction_amount: None,
            };

            let result = executor
                .run(
                    &cmd,
                    "bearer-owner".to_string(),
                    context("owner-test-1"),
                    &input,
                    Execute::Run,
                )
                .await
                .expect("owner should execute");

            match result {
                Executed::Done(output) => {
                    assert_eq!(output.message, "owner test");
                }
                _ => panic!("expected Done"),
            }
        });
    }
}

#[test]
fn agent_with_insufficient_tier_is_denied() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_test_table(&*kit.db).await;

            let auth = TestAuthenticator::new();
            let policy = DefaultPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let cmd = TestWriteCommand; // Requires Draft
            let input = TestInput {
                message: "should fail".to_string(),
                base_amount: None,
                transaction_amount: None,
            };

            let result = executor
                .run(
                    &cmd,
                    "bearer-read-agent".to_string(), // Read tier < Draft
                    context("tier-test-1"),
                    &input,
                    Execute::Run,
                )
                .await;

            assert!(result.is_err(), "should deny insufficient tier");
            match result {
                Err(CommandError::TierInsufficient { .. }) => {}
                _ => panic!("expected TierInsufficient"),
            }
        });
    }
}

#[test]
fn human_only_command_denied_to_agent() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_test_table(&*kit.db).await;

            let auth = TestAuthenticator::new();
            let policy = DefaultPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let cmd = TestHumanOnlyCommand;
            let input = TestInput {
                message: "agent attempt".to_string(),
                base_amount: None,
                transaction_amount: None,
            };

            let result = executor
                .run(
                    &cmd,
                    "bearer-draft-agent".to_string(),
                    context("human-test-1"),
                    &input,
                    Execute::Run,
                )
                .await;

            assert!(result.is_err(), "should deny agent");
            assert!(matches!(result, Err(CommandError::HumanOnly)));
        });
    }
}

#[test]
fn human_only_command_allowed_to_owner() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_test_table(&*kit.db).await;

            let auth = TestAuthenticator::new();
            let policy = DefaultPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let cmd = TestHumanOnlyCommand;
            let input = TestInput {
                message: "owner runs".to_string(),
                base_amount: None,
                transaction_amount: None,
            };

            let result = executor
                .run(
                    &cmd,
                    "bearer-owner".to_string(),
                    context("human-test-2"),
                    &input,
                    Execute::Run,
                )
                .await
                .expect("owner should execute human-only");

            assert!(matches!(result, Executed::Done(_)));
        });
    }
}

#[test]
fn agent_within_base_limits_executes() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_test_table(&*kit.db).await;

            let auth = TestAuthenticator::new();
            let policy = DefaultPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let cmd = TestPostCommand;
            let input = TestInput {
                message: "within limits".to_string(),
                base_amount: Some(50000), // EUR 500.00, within 1000.00 ceiling
                transaction_amount: None,
            };

            let result = executor
                .run(
                    &cmd,
                    "bearer-post-agent".to_string(),
                    context("within-test-1"),
                    &input,
                    Execute::Run,
                )
                .await
                .expect("should execute within limits");

            assert!(matches!(result, Executed::Done(_)));
        });
    }
}

#[test]
fn agent_over_base_ceiling_needs_approval() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_test_table(&*kit.db).await;

            let auth = TestAuthenticator::new();
            let policy = DefaultPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let cmd = TestPostCommand;
            let input = TestInput {
                message: "over base limit".to_string(),
                base_amount: Some(150000), // EUR 1500.00, exceeds 1000.00 ceiling
                transaction_amount: None,
            };

            let result = executor
                .run(
                    &cmd,
                    "bearer-post-agent".to_string(),
                    context("over-test-1"),
                    &input,
                    Execute::Run,
                )
                .await
                .expect("should return NeedsApproval");

            match result {
                Executed::NeedsApproval {
                    request_hash,
                    preview,
                    ..
                } => {
                    assert!(!request_hash.is_empty(), "request hash should be computed");
                    assert!(!preview.is_empty(), "preview should have lines");
                }
                _ => panic!("expected NeedsApproval"),
            }
        });
    }
}

#[test]
fn agent_with_no_ceiling_needs_approval() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_test_table(&*kit.db).await;

            let auth = TestAuthenticator::new();
            let policy = DefaultPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let cmd = TestPostCommand;
            let input = TestInput {
                message: "no limits".to_string(),
                base_amount: Some(1), // Any amount when ceiling is 0
                transaction_amount: None,
            };

            let result = executor
                .run(
                    &cmd,
                    "bearer-post-no-limits".to_string(),
                    context("no-limit-test-1"),
                    &input,
                    Execute::Run,
                )
                .await
                .expect("should return NeedsApproval");

            assert!(matches!(result, Executed::NeedsApproval { .. }));
        });
    }
}

#[test]
fn agent_within_base_but_over_transaction_ceiling_needs_approval() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_test_table(&*kit.db).await;

            let auth = TestAuthenticator::new();
            let policy = DefaultPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let cmd = TestWriteCommand;
            let input = TestInput {
                message: "within base, over txn".to_string(),
                base_amount: Some(60000), // EUR 600.00, within 1000.00 base ceiling
                transaction_amount: Some(60000), // USD 600.00, exceeds 500.00 txn ceiling
            };

            let result = executor
                .run(
                    &cmd,
                    "bearer-post-agent".to_string(),
                    context("txn-limit-test-1"),
                    &input,
                    Execute::Run,
                )
                .await
                .expect("should return NeedsApproval");

            assert!(matches!(result, Executed::NeedsApproval { .. }));
        });
    }
}

#[test]
fn dry_run_returns_preview_without_write() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_test_table(&*kit.db).await;

            let auth = TestAuthenticator::new();
            let policy = DefaultPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let cmd = TestWriteCommand;
            let input = TestInput {
                message: "dry run test".to_string(),
                base_amount: Some(10000),
                transaction_amount: None,
            };

            let result = executor
                .run(
                    &cmd,
                    "bearer-owner".to_string(),
                    context("dry-run-test-1"),
                    &input,
                    Execute::DryRun,
                )
                .await
                .expect("dry-run should succeed");

            match result {
                Executed::DryRun(preview) => {
                    assert!(!preview.is_empty(), "preview should have lines");
                }
                _ => panic!("expected DryRun"),
            }
        });
    }
}

#[test]
fn dry_run_unsupported_returns_error() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_test_table(&*kit.db).await;

            let auth = TestAuthenticator::new();
            let policy = DefaultPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let cmd = TestHumanOnlyCommand; // Does not support dry-run
            let input = TestInput {
                message: "unsupported dry-run".to_string(),
                base_amount: None,
                transaction_amount: None,
            };

            let result = executor
                .run(
                    &cmd,
                    "bearer-owner".to_string(),
                    context("unsupported-dry-run-test"),
                    &input,
                    Execute::DryRun,
                )
                .await;

            assert!(matches!(result, Err(CommandError::DryRunUnsupported)));
        });
    }
}

#[test]
fn bad_credential_returns_unauthenticated() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_test_table(&*kit.db).await;

            let auth = TestAuthenticator::new();
            let policy = DefaultPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let cmd = TestWriteCommand;
            let input = TestInput {
                message: "bad cred".to_string(),
                base_amount: None,
                transaction_amount: None,
            };

            let result = executor
                .run(
                    &cmd,
                    "bad-credential-xyz".to_string(),
                    context("bad-cred-test"),
                    &input,
                    Execute::Run,
                )
                .await;

            assert!(matches!(result, Err(CommandError::Unauthenticated)));
        });
    }
}

#[test]
fn read_command_with_empty_plan_succeeds() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_test_table(&*kit.db).await;

            let auth = TestAuthenticator::new();
            let policy = DefaultPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let cmd = TestReadCommand; // Read tier, empty plan
            let input = TestInput {
                message: "read test".to_string(),
                base_amount: None,
                transaction_amount: None,
            };

            let result = executor
                .run(
                    &cmd,
                    "bearer-owner".to_string(),
                    context("read-test-1"),
                    &input,
                    Execute::Run,
                )
                .await
                .expect("read should succeed");

            assert!(matches!(result, Executed::Done(_)));
        });
    }
}

#[test]
fn same_input_yields_same_approval_hash() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_test_table(&*kit.db).await;

            let auth = TestAuthenticator::new();
            let policy = DefaultPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let cmd = TestPostCommand;
            let input = TestInput {
                message: "same input".to_string(),
                base_amount: Some(150000), // Over limit
                transaction_amount: None,
            };

            let result1 = executor
                .run(
                    &cmd,
                    "bearer-post-agent".to_string(),
                    context("hash-test-1"),
                    &input,
                    Execute::Run,
                )
                .await;

            let result2 = executor
                .run(
                    &cmd,
                    "bearer-post-agent".to_string(),
                    context("hash-test-2"),
                    &input,
                    Execute::Run,
                )
                .await;

            let hash1 = match result1 {
                Ok(Executed::NeedsApproval { request_hash, .. }) => request_hash,
                _ => panic!("expected NeedsApproval"),
            };

            let hash2 = match result2 {
                Ok(Executed::NeedsApproval { request_hash, .. }) => request_hash,
                _ => panic!("expected NeedsApproval"),
            };

            assert_eq!(hash1, hash2, "same input should yield same hash");
        });
    }
}

#[test]
fn different_input_yields_different_hash() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            setup_test_table(&*kit.db).await;

            let auth = TestAuthenticator::new();
            let policy = DefaultPolicy;
            let executor = Executor::new(&*kit.db, auth, policy);

            let cmd = TestPostCommand;

            let input1 = TestInput {
                message: "input 1".to_string(),
                base_amount: Some(150000),
                transaction_amount: None,
            };

            let input2 = TestInput {
                message: "input 2".to_string(),
                base_amount: Some(150000),
                transaction_amount: None,
            };

            let result1 = executor
                .run(
                    &cmd,
                    "bearer-post-agent".to_string(),
                    context("diff-hash-test-1"),
                    &input1,
                    Execute::Run,
                )
                .await;

            let result2 = executor
                .run(
                    &cmd,
                    "bearer-post-agent".to_string(),
                    context("diff-hash-test-2"),
                    &input2,
                    Execute::Run,
                )
                .await;

            let hash1 = match result1 {
                Ok(Executed::NeedsApproval { request_hash, .. }) => request_hash,
                _ => panic!("expected NeedsApproval"),
            };

            let hash2 = match result2 {
                Ok(Executed::NeedsApproval { request_hash, .. }) => request_hash,
                _ => panic!("expected NeedsApproval"),
            };

            assert_ne!(hash1, hash2, "different input should yield different hash");
        });
    }
}
