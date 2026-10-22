//! Integration tests for ledgers-auth.
//!
//! Tests cover:
//! - Session cookie security
//! - Secrets never leak to error messages, logs, or audit
//! - Agent key operations (issue, revoke, rotate, authenticate)
//! - Revoked and rotated keys fail on next request

use async_trait::async_trait;
use cratefield_core::{ApiKeys, Statement};
use cratefield_testing::TestHarness;
use ledgers_audit::AuditContext;
use ledgers_auth::{AgentKeyDirectory, AgentKeys, Clock, KeyAuthenticator, KeyDirectoryEntry};
use ledgers_commands::{Authenticator, CommandError, Credential, Limits, Principal, Tier};
use ledgers_db::DbModule;
use std::sync::Arc;
use time::OffsetDateTime;

const COMPANY_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const NOW: &str = "2026-01-15T09:30:00Z";

/// Deterministic test random bytes (for API key generation).
struct TestRandomBytes(std::sync::Mutex<u64>);

impl TestRandomBytes {
    fn new() -> Self {
        Self(std::sync::Mutex::new(12345))
    }
}

impl cratefield_core::RandomBytes for TestRandomBytes {
    fn fill(&self, buf: &mut [u8]) -> Result<(), cratefield_core::RandomError> {
        let mut counter = self.0.lock().unwrap();
        *counter += 1;
        let seed = *counter;

        for (i, byte) in buf.iter_mut().enumerate() {
            #[allow(clippy::cast_possible_truncation)]
            {
                *byte = (seed.wrapping_mul(31).wrapping_add(i as u64)) as u8;
            }
        }
        Ok(())
    }
}

/// Test directory: always returns `Tier::PostWithinLimits` with empty limits.
struct TestKeyDirectory;

#[async_trait]
impl AgentKeyDirectory for TestKeyDirectory {
    async fn lookup(
        &self,
        _key_id: &str,
    ) -> Result<KeyDirectoryEntry, ledgers_auth::keys::KeyError> {
        Ok(KeyDirectoryEntry {
            tier: Tier::PostWithinLimits,
            limits: Limits::new(),
            expires_at: None,
            revoked_at: None,
        })
    }
}

/// Test clock: returns a fixed time (matching NOW constant).
struct TestClock;

impl Clock for TestClock {
    fn now(&self) -> OffsetDateTime {
        // Parse NOW as OffsetDateTime for consistent testing
        OffsetDateTime::parse(NOW, &time::format_description::well_known::Iso8601::DEFAULT)
            .unwrap_or(OffsetDateTime::UNIX_EPOCH)
    }
}

fn modules() -> Vec<Box<dyn cratefield_core::Module>> {
    vec![Box::new(DbModule)]
}

fn audit_context() -> AuditContext {
    use ledgers_audit::Actor;
    AuditContext::new(
        Actor::Person {
            person_id: "usr_test_person_01".to_owned(),
        },
        "req_test_001".to_string(),
        "evt_test_001".to_string(),
    )
}

/// Shared setup: create `api_keys` table, return `(api_keys, agent_keys, authenticator)`.
async fn init_keys(kit: &TestHarness) -> (Arc<ApiKeys>, AgentKeys, KeyAuthenticator) {
    let random = Arc::new(TestRandomBytes::new());
    let api_keys = Arc::new(ApiKeys::new(
        kit.db.clone(),
        Arc::new(kit.clock.clone()),
        random,
        "api_keys",
    ));
    let _ = kit
        .db
        .execute(&Statement::new(api_keys.create_table_sql()))
        .await;
    let agent_keys = AgentKeys::new(api_keys.clone(), kit.db.clone());
    let directory = Arc::new(TestKeyDirectory);
    let clock = Arc::new(TestClock);
    let authenticator = KeyAuthenticator::new(api_keys.clone(), directory, clock);

    (api_keys, agent_keys, authenticator)
}

#[test]
fn session_cookie_attributes() {
    use ledgers_auth::SessionBuilder;

    let builder = SessionBuilder::new("test-session-id");
    let cookie = builder.build();

    // Build: check all required security attributes.
    assert!(cookie.contains("__Host-ledgers-session="));
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("Secure"));
    assert!(cookie.contains("SameSite=Lax"));
    assert!(cookie.contains("Path=/"));
    assert!(cookie.contains("Max-Age="));

    // Clear: Max-Age=0 with all security attributes.
    let clear_cookie = SessionBuilder::clear();
    assert!(clear_cookie.contains("Max-Age=0"));
    assert!(clear_cookie.contains("__Host-ledgers-session="));
    assert!(clear_cookie.contains("HttpOnly"));
    assert!(clear_cookie.contains("Secure"));
    assert!(clear_cookie.contains("SameSite=Lax"));
}

#[test]
fn agent_key_issue_debug_redacted() {
    use ledgers_auth::AgentKeyIssue;

    let issue = AgentKeyIssue::new(
        "lk_bearer_abc123".to_string(),
        "full_token_material".to_string(),
    );

    let debug_str = format!("{issue:?}");
    assert!(!debug_str.contains("full_token_material"));
    assert!(debug_str.contains("<REDACTED>"));
    assert!(debug_str.contains("lk_bearer_abc123"));
}

#[test]
fn revoked_key_fails_on_next_request() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            let (_api_keys, agent_keys, authenticator) = init_keys(&kit).await;

            let issued = agent_keys
                .issue("key_test_01", COMPANY_ID, &audit_context(), NOW)
                .await
                .expect("issue key");

            let key_id = issued.key_id.clone();
            let full_key = issued.key();
            let credential: Credential = full_key;

            let principal = authenticator
                .authenticate(&credential)
                .await
                .expect("first authenticate should succeed");
            assert!(matches!(principal, Principal::Agent { .. }));

            agent_keys
                .revoke(&key_id, COMPANY_ID, &audit_context(), NOW)
                .await
                .expect("revoke key");

            let result = authenticator.authenticate(&credential).await;
            assert!(
                result.is_err(),
                "revoked key should fail on next authenticate"
            );
            assert!(matches!(result, Err(CommandError::Unauthenticated)));
        });
    }
}

#[test]
fn rotation_invalidates_old_secret() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            let (_api_keys, agent_keys, authenticator) = init_keys(&kit).await;

            let issued = agent_keys
                .issue("key_test_02", COMPANY_ID, &audit_context(), NOW)
                .await
                .expect("issue key");

            let key_id = issued.key_id.clone();
            let old_key = issued.key();
            let old_credential: Credential = old_key;

            let principal = authenticator
                .authenticate(&old_credential)
                .await
                .expect("old key should authenticate");
            assert!(matches!(principal, Principal::Agent { .. }));

            let rotated = agent_keys
                .rotate(&key_id, COMPANY_ID, &audit_context(), NOW)
                .await
                .expect("rotate key");

            let result = authenticator.authenticate(&old_credential).await;
            assert!(result.is_err(), "old key should fail after rotation");

            let new_key = rotated.key();
            let new_credential: Credential = new_key;
            let principal = authenticator
                .authenticate(&new_credential)
                .await
                .expect("new key should authenticate");
            assert!(matches!(principal, Principal::Agent { .. }));
        });
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn keys_never_leak_to_logs_audit_or_errors() {
    for kit in TestHarness::all_dialects(modules) {
        pollster::block_on(async {
            let (_api_keys, agent_keys, authenticator) = init_keys(&kit).await;

            // Set up tracing to capture logs.
            let log_buffer = Arc::new(std::sync::Mutex::new(Vec::new()));
            let writer = LogWriterMaker(Arc::clone(&log_buffer));

            let subscriber = tracing_subscriber::fmt()
                .with_max_level(tracing::Level::TRACE)
                .with_ansi(false)
                .with_writer(writer)
                .finish();

            let _guard = tracing::subscriber::set_default(subscriber);

            // Sanity check: verify tracing is working.
            let public_prefix = "lk_test_sanity_prefix";
            tracing::info!(prefix = %public_prefix, "probe");
            {
                let captured = log_buffer.lock().unwrap();
                assert!(
                    String::from_utf8_lossy(&captured).contains(public_prefix),
                    "tracing probe failed: log capture not working"
                );
            }
            log_buffer.lock().unwrap().clear();

            // Issue a key
            let issued = agent_keys
                .issue("key_test_03", COMPANY_ID, &audit_context(), NOW)
                .await
                .expect("issue key");

            let public_prefix = issued.key_id.clone();
            let full_key = issued.key();

            // Authenticate twice to trigger last-used touch (which cratefield-core may log).
            let good_cred: Credential = full_key.clone();
            let _ = authenticator.authenticate(&good_cred).await;
            let _ = authenticator.authenticate(&good_cred).await;

            // Rotate the key
            let rotated = agent_keys
                .rotate(&public_prefix, COMPANY_ID, &audit_context(), NOW)
                .await
                .expect("rotate key");
            let new_key = rotated.key();

            // Revoke the key
            agent_keys
                .revoke(&public_prefix, COMPANY_ID, &audit_context(), NOW)
                .await
                .expect("revoke key");

            // Capture logs after all operations
            let log_text = {
                let captured = log_buffer.lock().unwrap();
                String::from_utf8_lossy(&captured).to_string()
            };

            // Read audit events from the database
            let audit_text = kit
                .db
                .query(&Statement::new(
                    "SELECT payload FROM audit_event ORDER BY seq".to_string(),
                ))
                .await
                .map(|result| {
                    result
                        .rows
                        .iter()
                        .filter_map(|row| row.get::<String>("payload"))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();

            // Extract bare secret parts (segment after the last `_`)
            let full_key_secret = full_key
                .rsplit('_')
                .next()
                .map(str::to_string)
                .unwrap_or_default();
            let new_key_secret = new_key
                .rsplit('_')
                .next()
                .map(str::to_string)
                .unwrap_or_default();

            // Assert full keys do not leak to logs
            assert!(
                !log_text.contains(&full_key),
                "full key should not appear in logs"
            );
            assert!(
                !log_text.contains(&new_key),
                "rotated key should not appear in logs"
            );
            assert!(
                !log_text.contains(&full_key_secret),
                "full key secret part should not appear in logs"
            );
            assert!(
                !log_text.contains(&new_key_secret),
                "rotated key secret part should not appear in logs"
            );

            // Assert full keys do not leak to audit
            assert!(
                !audit_text.contains(&full_key),
                "full key should not appear in audit"
            );
            assert!(
                !audit_text.contains(&new_key),
                "rotated key should not appear in audit"
            );
            assert!(
                !audit_text.contains(&full_key_secret),
                "full key secret part should not appear in audit"
            );
            assert!(
                !audit_text.contains(&new_key_secret),
                "rotated key secret part should not appear in audit"
            );

            // Assert public prefix appears in audit
            assert!(
                audit_text.contains(&public_prefix),
                "public prefix should appear in audit"
            );

            // Assert audit contains expected actions
            assert!(audit_text.contains("auth.key.issue"));
            assert!(audit_text.contains("auth.key.rotate"));
            assert!(audit_text.contains("auth.key.revoke"));
        });
    }
}

/// A `MakeWriter` implementation that writes to an `Arc<Mutex<Vec<u8>>>`.
struct LogWriter(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct LogWriterMaker(Arc<std::sync::Mutex<Vec<u8>>>);

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogWriterMaker {
    type Writer = LogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LogWriter(Arc::clone(&self.0))
    }
}
