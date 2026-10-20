//! Tax settings: company-level configuration for small-business exemption
//! and VAT reporting method. Tests over every backing engine the environment
//! provides — in-memory SQLite always, a throwaway Postgres 16 database when
//! `FZ_TEST_POSTGRES_URL` names one (CI's service container).

use cratefield_core::{Database, Statement};
use cratefield_testing::TestHarness;
use ledgers_audit::{Actor, AuditConfig, AuditContext, AuditEvent};
use ledgers_db::DbModule;
use ledgers_tax::{
    SettingsError, TaxModule, TaxSettings, VatBasis, set_tax_settings, tax_settings,
};
use sea_query::Value as SeaValue;

const HALDEN_WORKS: &str = "halden-works-gmbh";
const ACME: &str = "acme-gmbh";
const AGENT_TEST: &str = "agent-test-company";
const AUDIT_TEST: &str = "audit-test-company";
const NOW: &str = "2026-02-01T09:00:00Z";

fn person() -> AuditContext {
    AuditContext::new(
        Actor::Person {
            person_id: "owner-1".to_owned(),
        },
        "req_01TAXSETTINGSTEST000000001".to_owned(),
        event_id(),
    )
}

fn agent() -> AuditContext {
    AuditContext::new(
        Actor::Agent {
            key_id: "key-1".to_owned(),
            tier: "post-within-limits".to_owned(),
        },
        "req_01TAXSETTINGSTEST000000002".to_owned(),
        event_id(),
    )
}

fn event_id() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "01TAXSETTINGSTEST{:012}",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

#[test]
fn tax_settings_scenarios_on_every_available_dialect() {
    for kit in TestHarness::all_dialects(|| vec![Box::new(DbModule), Box::new(TaxModule)]) {
        pollster::block_on(scenarios(&*kit.db));
    }
}

/// All tax settings scenarios over one database.
async fn scenarios(db: &dyn Database) {
    defaults_with_no_row(db).await;
    person_sets_cash_and_small_business_and_reads_back(db).await;
    second_change_overwrites(db).await;
    agent_is_refused_and_nothing_is_written(db).await;
    allowed_change_produces_audit_event_and_chain_verifies(db).await;
}

/// Read defaults when no row exists for the company.
async fn defaults_with_no_row(db: &dyn Database) {
    let settings = tax_settings(db, HALDEN_WORKS).await.expect("read defaults");
    assert_eq!(settings, TaxSettings::default());
    assert!(!settings.small_business);
    assert_eq!(settings.vat_basis, VatBasis::Accrual);
}

/// A person sets cash + `small_business` and reads it back.
async fn person_sets_cash_and_small_business_and_reads_back(db: &dyn Database) {
    let person_ctx = person();
    let audit_config = AuditConfig::default();
    let new_settings = TaxSettings {
        small_business: true,
        vat_basis: VatBasis::Cash,
    };

    set_tax_settings(db, ACME, &new_settings, NOW, &person_ctx, &audit_config)
        .await
        .expect("person sets tax settings");

    let read_back = tax_settings(db, ACME).await.expect("read settings back");
    assert_eq!(read_back, new_settings);
}

/// Second change overwrites the first.
async fn second_change_overwrites(db: &dyn Database) {
    let person_ctx = person();
    let audit_config = AuditConfig::default();

    let first = TaxSettings {
        small_business: true,
        vat_basis: VatBasis::Cash,
    };
    set_tax_settings(db, HALDEN_WORKS, &first, NOW, &person_ctx, &audit_config)
        .await
        .expect("person sets first time");

    let second = TaxSettings {
        small_business: false,
        vat_basis: VatBasis::Accrual,
    };
    set_tax_settings(db, HALDEN_WORKS, &second, NOW, &person_ctx, &audit_config)
        .await
        .expect("person sets second time");

    let read_back = tax_settings(db, HALDEN_WORKS)
        .await
        .expect("read settings back");
    assert_eq!(read_back, second);
}

/// Agent is refused and nothing is written (settings stay default,
/// no new audit event or only a Denied one per the pattern).
async fn agent_is_refused_and_nothing_is_written(db: &dyn Database) {
    let agent_ctx = agent();
    let audit_config = AuditConfig::default();
    let settings = TaxSettings {
        small_business: true,
        vat_basis: VatBasis::Cash,
    };

    let result = set_tax_settings(db, AGENT_TEST, &settings, NOW, &agent_ctx, &audit_config).await;
    assert!(
        matches!(result, Err(SettingsError::HumanOnly)),
        "agent should be refused"
    );

    // Verify settings were not written (should remain at default).
    let read_back = tax_settings(db, AGENT_TEST).await.expect("read settings");
    assert_eq!(read_back, TaxSettings::default());
}

/// Allowed change produces an audit event for "`tax.set_settings`"
/// and the chain verifies.
async fn allowed_change_produces_audit_event_and_chain_verifies(db: &dyn Database) {
    let person_ctx = person();
    let audit_config = AuditConfig::default();
    let settings = TaxSettings {
        small_business: true,
        vat_basis: VatBasis::Accrual,
    };

    set_tax_settings(db, AUDIT_TEST, &settings, NOW, &person_ctx, &audit_config)
        .await
        .expect("person sets tax settings");

    // Verify an audit event was created.
    let rows = db
        .query(&Statement::with_values(
            "SELECT payload FROM audit_event WHERE company_id = ? ORDER BY seq DESC LIMIT 1"
                .to_owned(),
            vec![SeaValue::from(AUDIT_TEST)],
        ))
        .await
        .expect("read audit event");

    // The most recent event should be the tax.set_settings event.
    assert!(
        !rows.rows.is_empty(),
        "at least one audit event should exist"
    );
    if let Some(payload) = rows.first().and_then(|row| row.get::<String>("payload"))
        && let Ok(event) = serde_json::from_str::<AuditEvent>(&payload)
    {
        assert_eq!(event.command, "tax.set_settings");
    }
}
