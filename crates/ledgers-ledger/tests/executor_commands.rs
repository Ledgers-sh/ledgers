//! Tests for ledgers-ledger command executor integration: account creation,
//! entry posting, policy enforcement, and idempotency through the executor.

mod common;
use common::*;
use cratefield_core::Database;
use ledgers_commands::auth::{Authenticator, Credential, Limit, Limits, Principal, Tier};
use ledgers_commands::error::CommandError;
use ledgers_commands::policy::{Decision, Policy};
use ledgers_commands::registry::CommandMeta;
use ledgers_commands::{CommandContext, Executed, Executor, JournalPreview};
use ledgers_ledger::surface::{CreateAccount, CreateAccountInput, PostEntry, PostEntryInput};
use ledgers_ledger::surface::{
    SerializableAccountDraft, SerializableAmount, SerializableEntryDraft, SerializableLine,
};
use std::collections::BTreeMap;

/// A tiny test authenticator that grants access based on simple rules.
struct TestAuthenticator {
    owner: bool,
    tier: Tier,
    limits: Limits,
}

impl TestAuthenticator {
    fn owner() -> Self {
        TestAuthenticator {
            owner: true,
            tier: Tier::PostWithinLimits,
            limits: Limits::new(),
        }
    }

    fn agent_with_eur_limit(tier: Tier, base_ceiling: i128) -> Self {
        let limits = Limits::new()
            .insert(
                "EUR",
                Limit {
                    base_ceiling,
                    transaction_ceiling: i128::MAX,
                },
            )
            .unwrap();
        TestAuthenticator {
            owner: false,
            tier,
            limits,
        }
    }
}

#[async_trait::async_trait]
impl Authenticator for TestAuthenticator {
    async fn authenticate(&self, _credential: &Credential) -> Result<Principal, CommandError> {
        if self.owner {
            Ok(Principal::Person {
                person_id: "test_person".to_string(),
                role: ledgers_commands::Role::Owner,
            })
        } else {
            Ok(Principal::Agent {
                key_id: "test_agent".to_string(),
                tier: self.tier,
                limits: self.limits.clone(),
            })
        }
    }
}

/// A test policy that always allows owners and checks agent tiers/limits.
struct TestPolicy;

impl Policy for TestPolicy {
    fn decide(
        &self,
        principal: &Principal,
        meta: &CommandMeta,
        preview: &JournalPreview,
    ) -> Decision {
        match principal {
            Principal::Person { .. } => Decision::Allow,
            Principal::Agent { tier, limits, .. } => {
                if meta.human_only {
                    return Decision::Deny {
                        reason: "command is human-only".to_string(),
                    };
                }

                if *tier < meta.tier {
                    return Decision::Deny {
                        reason: format!("tier insufficient for {}", meta.tier),
                    };
                }

                if preview.is_empty() {
                    return Decision::Allow;
                }

                // Check limits: sum debit-side amounts per currency and check against ceiling.
                // Canonical format is major.minor (e.g., "100.00" for EUR), so we extract
                // the major part only to compare with the limit.
                for (currency, total_str) in &preview.totals {
                    let limit = limits.get(currency.as_str()).map_or(0, |l| l.base_ceiling);

                    // Parse major part of canonical string (before the decimal point).
                    let major = total_str
                        .split('.')
                        .next()
                        .and_then(|s| s.parse::<i128>().ok())
                        .unwrap_or(0);

                    if major > limit {
                        return Decision::NeedsApproval {
                            reason: format!("amount {total_str} exceeds limit {limit}"),
                        };
                    }
                }

                Decision::Allow
            }
        }
    }
}

/// Creates a test journal with standard properties.
async fn create_test_journal(db: &dyn Database, company_id: &str, journal_id: &str) {
    db.execute(&cratefield_core::Statement::with_values(
        "INSERT INTO journal (id, company_id, code, kind, name, created_at) VALUES (?, ?, ?, ?, ?, ?)".to_owned(),
        vec![
            sea_query::Value::from(journal_id),
            sea_query::Value::from(company_id),
            sea_query::Value::from("GN"),
            sea_query::Value::from("general"),
            sea_query::Value::from("Gen"),
            sea_query::Value::from(NOW),
        ],
    ))
    .await
    .unwrap();
}

#[test]
fn owner_creates_account_succeeds() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;

        let executor = Executor::new(db, TestAuthenticator::owner(), TestPolicy);

        let input = CreateAccountInput {
            draft: SerializableAccountDraft {
                number: "5000".to_string(),
                name_key: "account.test".to_string(),
                account_type: "income".to_string(),
                normal_balance: Some("credit".to_string()),
                currency: None,
                parent: None,
                tags: vec![],
            },
        };

        let result = executor
            .run(
                &CreateAccount,
                "token".to_string(),
                CommandContext {
                    company_id: COMPANY_ID.to_string(),
                    request_id: "req_001".to_string(),
                    now: NOW.to_string(),
                    event_id: "evt_001".to_string(),
                },
                &input,
                ledgers_commands::Execute::Run,
            )
            .await;

        if let Err(ref e) = result {
            eprintln!("Error: {e:?}");
        }
        assert!(result.is_ok());
        match result.unwrap() {
            Executed::Done(output) => {
                assert_eq!(output.account.number, "5000");
            }
            _ => panic!("expected Done"),
        }

        // Verify the account was created
        let count = count(db, "account", "number", "5000").await;
        assert_eq!(count, 1);
    });
}

#[test]
fn owner_posts_balanced_entry_succeeds() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;
        open_period(db, COMPANY_ID, "2026-02").await;
        create_test_journal(db, COMPANY_ID, "test-journal").await;

        let executor = Executor::new(db, TestAuthenticator::owner(), TestPolicy);

        let input = PostEntryInput {
            journal_id: "test-journal".to_string(),
            fiscal_year: FY,
            entry: SerializableEntryDraft {
                journal_id: "test-journal".to_string(),
                document_date: "2026-02-01".to_string(),
                posting_date: "2026-02-01".to_string(),
                tax_point_date: None,
                base_currency: "EUR".to_string(),
                description: "test post".to_string(),
                source_documents: vec![],
                external_id: None,
                source_app: None,
                reverses: None,
                lines: vec![
                    SerializableLine {
                        account_number: "1010".to_string(),
                        side: "debit".to_string(),
                        base: SerializableAmount {
                            minor: "10000".to_string(),
                            currency: "EUR".to_string(),
                        },
                        fx: None,
                        tax_code: None,
                        dimensions: BTreeMap::new(),
                        memo: None,
                    },
                    SerializableLine {
                        account_number: "1400".to_string(),
                        side: "credit".to_string(),
                        base: SerializableAmount {
                            minor: "10000".to_string(),
                            currency: "EUR".to_string(),
                        },
                        fx: None,
                        tax_code: None,
                        dimensions: BTreeMap::new(),
                        memo: None,
                    },
                ],
            },
        };

        let result = executor
            .run(
                &PostEntry,
                "token".to_string(),
                CommandContext {
                    company_id: COMPANY_ID.to_string(),
                    request_id: "req_002".to_string(),
                    now: NOW.to_string(),
                    event_id: "evt_002".to_string(),
                },
                &input,
                ledgers_commands::Execute::Run,
            )
            .await;

        if let Err(ref e) = result {
            eprintln!("Post Error: {e:?}");
        }
        assert!(result.is_ok());
        match result.unwrap() {
            Executed::Done(output) => {
                assert_eq!(output.entry.number, 1);
                assert_eq!(output.entry.fiscal_year, FY);
            }
            _ => panic!("expected Done"),
        }

        // Verify the entry was posted
        let count = count(db, "journal_entry", "journal_id", "test-journal").await;
        assert_eq!(count, 1);
    });
}

#[test]
fn agent_exceeding_limit_needs_approval() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;
        open_period(db, COMPANY_ID, "2026-02").await;
        create_test_journal(db, COMPANY_ID, "test-journal").await;

        // Agent with 5000 EUR limit
        let executor = Executor::new(
            db,
            TestAuthenticator::agent_with_eur_limit(Tier::PostWithinLimits, 5_000),
            TestPolicy,
        );

        let input = PostEntryInput {
            journal_id: "test-journal".to_string(),
            fiscal_year: FY,
            entry: SerializableEntryDraft {
                journal_id: "test-journal".to_string(),
                document_date: "2026-02-01".to_string(),
                posting_date: "2026-02-01".to_string(),
                tax_point_date: None,
                base_currency: "EUR".to_string(),
                description: "over limit".to_string(),
                source_documents: vec![],
                external_id: None,
                source_app: None,
                reverses: None,
                lines: vec![
                    SerializableLine {
                        account_number: "1010".to_string(),
                        side: "debit".to_string(),
                        base: SerializableAmount {
                            minor: "600000".to_string(),
                            currency: "EUR".to_string(),
                        },
                        fx: None,
                        tax_code: None,
                        dimensions: BTreeMap::new(),
                        memo: None,
                    },
                    SerializableLine {
                        account_number: "1400".to_string(),
                        side: "credit".to_string(),
                        base: SerializableAmount {
                            minor: "600000".to_string(),
                            currency: "EUR".to_string(),
                        },
                        fx: None,
                        tax_code: None,
                        dimensions: BTreeMap::new(),
                        memo: None,
                    },
                ],
            },
        };

        let result = executor
            .run(
                &PostEntry,
                "token".to_string(),
                CommandContext {
                    company_id: COMPANY_ID.to_string(),
                    request_id: "req_003".to_string(),
                    now: NOW.to_string(),
                    event_id: "evt_003".to_string(),
                },
                &input,
                ledgers_commands::Execute::Run,
            )
            .await;

        if let Err(ref e) = result {
            eprintln!("Limit test error: {e:?}");
        }
        assert!(result.is_ok());
        match result.unwrap() {
            Executed::NeedsApproval { .. } => {
                // Expected
            }
            _ => panic!("expected NeedsApproval"),
        }

        // Verify nothing was posted
        let count = count(db, "journal_entry", "journal_id", "test-journal").await;
        assert_eq!(count, 0);

        // Verify counter was not consumed
        assert_eq!(counter_value(db, "test-journal", FY).await, None);
    });
}

#[test]
fn dryrun_returns_preview_without_posting() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;
        open_period(db, COMPANY_ID, "2026-02").await;
        create_test_journal(db, COMPANY_ID, "test-journal").await;

        let executor = Executor::new(db, TestAuthenticator::owner(), TestPolicy);

        let input = PostEntryInput {
            journal_id: "test-journal".to_string(),
            fiscal_year: FY,
            entry: SerializableEntryDraft {
                journal_id: "test-journal".to_string(),
                document_date: "2026-02-01".to_string(),
                posting_date: "2026-02-01".to_string(),
                tax_point_date: None,
                base_currency: "EUR".to_string(),
                description: "dry run test".to_string(),
                source_documents: vec![],
                external_id: None,
                source_app: None,
                reverses: None,
                lines: vec![
                    SerializableLine {
                        account_number: "1010".to_string(),
                        side: "debit".to_string(),
                        base: SerializableAmount {
                            minor: "5000".to_string(),
                            currency: "EUR".to_string(),
                        },
                        fx: None,
                        tax_code: None,
                        dimensions: BTreeMap::new(),
                        memo: None,
                    },
                    SerializableLine {
                        account_number: "1400".to_string(),
                        side: "credit".to_string(),
                        base: SerializableAmount {
                            minor: "5000".to_string(),
                            currency: "EUR".to_string(),
                        },
                        fx: None,
                        tax_code: None,
                        dimensions: BTreeMap::new(),
                        memo: None,
                    },
                ],
            },
        };

        let result = executor
            .run(
                &PostEntry,
                "token".to_string(),
                CommandContext {
                    company_id: COMPANY_ID.to_string(),
                    request_id: "req_005".to_string(),
                    now: NOW.to_string(),
                    event_id: "evt_005".to_string(),
                },
                &input,
                ledgers_commands::Execute::DryRun,
            )
            .await;

        assert!(result.is_ok());
        match result.unwrap() {
            Executed::DryRun(preview) => {
                assert!(!preview.is_empty());
            }
            _ => panic!("expected DryRun"),
        }

        // Verify nothing was posted
        let count = count(db, "journal_entry", "journal_id", "test-journal").await;
        assert_eq!(count, 0);
    });
}

#[test]
fn agent_with_draft_tier_denied() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;
        open_period(db, COMPANY_ID, "2026-02").await;
        create_test_journal(db, COMPANY_ID, "test-journal").await;

        // Agent with Draft tier (insufficient for PostWithinLimits command)
        let executor = Executor::new(
            db,
            TestAuthenticator::agent_with_eur_limit(Tier::Draft, 10_000),
            TestPolicy,
        );

        let input = PostEntryInput {
            journal_id: "test-journal".to_string(),
            fiscal_year: FY,
            entry: SerializableEntryDraft {
                journal_id: "test-journal".to_string(),
                document_date: "2026-02-01".to_string(),
                posting_date: "2026-02-01".to_string(),
                tax_point_date: None,
                base_currency: "EUR".to_string(),
                description: "draft agent post".to_string(),
                source_documents: vec![],
                external_id: None,
                source_app: None,
                reverses: None,
                lines: vec![
                    SerializableLine {
                        account_number: "1010".to_string(),
                        side: "debit".to_string(),
                        base: SerializableAmount {
                            minor: "10000".to_string(),
                            currency: "EUR".to_string(),
                        },
                        fx: None,
                        tax_code: None,
                        dimensions: BTreeMap::new(),
                        memo: None,
                    },
                    SerializableLine {
                        account_number: "1400".to_string(),
                        side: "credit".to_string(),
                        base: SerializableAmount {
                            minor: "10000".to_string(),
                            currency: "EUR".to_string(),
                        },
                        fx: None,
                        tax_code: None,
                        dimensions: BTreeMap::new(),
                        memo: None,
                    },
                ],
            },
        };

        let result = executor
            .run(
                &PostEntry,
                "token".to_string(),
                CommandContext {
                    company_id: COMPANY_ID.to_string(),
                    request_id: "req_004".to_string(),
                    now: NOW.to_string(),
                    event_id: "evt_004".to_string(),
                },
                &input,
                ledgers_commands::Execute::Run,
            )
            .await;

        // Should get a policy denied error
        assert!(result.is_err());

        // Verify nothing was posted
        let count = count(db, "journal_entry", "journal_id", "test-journal").await;
        assert_eq!(count, 0);

        // Verify counter was not consumed
        assert_eq!(counter_value(db, "test-journal", FY).await, None);
    });
}

#[test]
fn idempotent_replay_same_entry_id() {
    let harness = harness();
    pollster::block_on(async {
        let db = harness.db.as_ref();
        mount(db, COMPANY_ID).await;
        open_period(db, COMPANY_ID, "2026-02").await;
        create_test_journal(db, COMPANY_ID, "test-journal").await;

        let executor = Executor::new(db, TestAuthenticator::owner(), TestPolicy);

        let input = PostEntryInput {
            journal_id: "test-journal".to_string(),
            fiscal_year: FY,
            entry: SerializableEntryDraft {
                journal_id: "test-journal".to_string(),
                document_date: "2026-02-01".to_string(),
                posting_date: "2026-02-01".to_string(),
                tax_point_date: None,
                base_currency: "EUR".to_string(),
                description: "idempotent test".to_string(),
                source_documents: vec![],
                external_id: None,
                source_app: None,
                reverses: None,
                lines: vec![
                    SerializableLine {
                        account_number: "1010".to_string(),
                        side: "debit".to_string(),
                        base: SerializableAmount {
                            minor: "50000".to_string(),
                            currency: "EUR".to_string(),
                        },
                        fx: None,
                        tax_code: None,
                        dimensions: BTreeMap::new(),
                        memo: None,
                    },
                    SerializableLine {
                        account_number: "1400".to_string(),
                        side: "credit".to_string(),
                        base: SerializableAmount {
                            minor: "50000".to_string(),
                            currency: "EUR".to_string(),
                        },
                        fx: None,
                        tax_code: None,
                        dimensions: BTreeMap::new(),
                        memo: None,
                    },
                ],
            },
        };

        // First post
        let result1 = executor
            .run(
                &PostEntry,
                "token".to_string(),
                CommandContext {
                    company_id: COMPANY_ID.to_string(),
                    request_id: "req_006".to_string(),
                    now: NOW.to_string(),
                    event_id: "evt_006".to_string(),
                },
                &input,
                ledgers_commands::Execute::Run,
            )
            .await;

        assert!(result1.is_ok());
        let number1 = match result1.unwrap() {
            Executed::Done(output) => output.entry.number,
            _ => panic!("expected Done"),
        };

        // Verify entry was posted
        let count_initial = count(db, "journal_entry", "journal_id", "test-journal").await;
        assert_eq!(count_initial, 1);

        // Second post with same input and same request_id (idempotent replay)
        let result2 = executor
            .run(
                &PostEntry,
                "token".to_string(),
                CommandContext {
                    company_id: COMPANY_ID.to_string(),
                    request_id: "req_006".to_string(), // Same request ID for idempotency
                    now: NOW.to_string(),
                    event_id: "evt_006b".to_string(), // Different event ID is OK
                },
                &input,
                ledgers_commands::Execute::Run,
            )
            .await;

        assert!(result2.is_ok());
        let number2 = match result2.unwrap() {
            Executed::Done(output) => output.entry.number,
            _ => panic!("expected Done"),
        };

        // Should get the same number
        assert_eq!(number1, number2);

        // Should still have only one entry (idempotent)
        let count_after_replay = count(db, "journal_entry", "journal_id", "test-journal").await;
        assert_eq!(count_after_replay, 1);

        // Counter should not have advanced further
        let counter_val = counter_value(db, "test-journal", FY).await;
        assert_eq!(counter_val, Some(1));
    });
}
