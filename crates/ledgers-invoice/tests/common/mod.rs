//! Shared test utilities for invoice and credit note tests.

use cratefield_core::{Database, Statement};
use ledgers_commands::auth::{Authenticator, Credential, Principal, Role};
use ledgers_commands::{
    Command, CommandContext, CommandError, DefaultPolicy, Execute, Executed, Executor,
};
use sea_query::Value;
use std::collections::BTreeMap;

pub const COMPANY_ID: &str = "01TESTCOMPANY0000000000000A";
pub const NOW: &str = "2026-10-24T09:30:00Z";
pub const OWNER_TOKEN: &str = "bearer-owner";

pub struct TestAuthenticator {
    credentials: BTreeMap<String, Principal>,
}

impl TestAuthenticator {
    pub fn new() -> Self {
        let mut credentials = BTreeMap::new();
        credentials.insert(
            OWNER_TOKEN.to_string(),
            Principal::Person {
                person_id: "owner-test".to_string(),
                role: Role::Owner,
            },
        );
        TestAuthenticator { credentials }
    }

    #[allow(dead_code)]
    pub fn with_agent_key(
        mut self,
        token: String,
        tier: ledgers_commands::auth::Tier,
        limits: ledgers_commands::Limits,
    ) -> Self {
        self.credentials.insert(
            token,
            Principal::Agent {
                key_id: "agent-key".to_string(),
                tier,
                limits,
            },
        );
        self
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

pub async fn run_command<C: Command>(
    db: &dyn Database,
    cmd: &C,
    input: &C::Input,
    request_id: &str,
) -> Result<C::Output, CommandError> {
    let executor = Executor::new(db, TestAuthenticator::new(), DefaultPolicy);
    match executor
        .run(
            cmd,
            OWNER_TOKEN.to_string(),
            CommandContext {
                company_id: COMPANY_ID.to_string(),
                request_id: request_id.to_string(),
                event_id: format!("{request_id}-event"),
                now: NOW.to_string(),
                is_person: true,
            },
            input,
            Execute::Run,
        )
        .await?
    {
        Executed::Done(output) => Ok(output),
        _ => Err(CommandError::InvalidInput("unexpected state".into())),
    }
}

#[allow(dead_code)]
pub async fn run_command_as_agent<C: Command>(
    db: &dyn Database,
    cmd: &C,
    input: &C::Input,
    request_id: &str,
    agent_token: String,
    authenticator: TestAuthenticator,
) -> Result<C::Output, CommandError> {
    let executor = Executor::new(db, authenticator, DefaultPolicy);
    match executor
        .run(
            cmd,
            agent_token,
            CommandContext {
                company_id: COMPANY_ID.to_string(),
                request_id: request_id.to_string(),
                event_id: format!("{request_id}-event"),
                now: NOW.to_string(),
                is_person: false,
            },
            input,
            Execute::Run,
        )
        .await?
    {
        Executed::Done(output) => Ok(output),
        _ => Err(CommandError::InvalidInput("unexpected state".into())),
    }
}

async fn exec(db: &dyn Database, sql: &str, values: Vec<Value>) {
    db.execute(&Statement::with_values(sql.to_string(), values))
        .await
        .unwrap_or_else(|e| panic!("setup failed: {e}"));
}

async fn seed_accounts(db: &dyn Database) {
    let accounts = vec![
        ("1200", "ar_control", "asset", "debit", "ar"),
        ("2100", "vat_output", "liability", "credit", "vat-output"),
        ("1050", "vat_input", "asset", "debit", "vat-input"),
        ("4000", "revenue", "income", "credit", ""),
        ("1010", "bank", "asset", "debit", "bank"),
        ("8100", "fx_gain", "income", "credit", "fx-gain"),
        ("8200", "fx_loss", "expense", "debit", "fx-loss"),
    ];
    for (num, key, typ, balance, tag) in accounts {
        exec(
            db,
            "INSERT INTO account (company_id, number, name_key, account_type, normal_balance, tags, archived, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, 0, ?, ?)",
            vec![
                Value::from(COMPANY_ID),
                Value::from(num),
                Value::from(key),
                Value::from(typ),
                Value::from(balance),
                Value::from(tag),
                Value::from(NOW),
                Value::from(NOW),
            ],
        )
        .await;
    }
}

#[allow(dead_code)]
async fn seed_company_base(db: &dyn Database, periods: &[&str]) {
    exec(
        db,
        "INSERT INTO company (id, name, base_currency, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
        vec![
            Value::from(COMPANY_ID),
            Value::from("Halden Works GmbH"),
            Value::from("EUR"),
            Value::from(NOW),
            Value::from(NOW),
        ],
    )
    .await;
    exec(
        db,
        "INSERT INTO company_setting (company_id, key, value, updated_at) VALUES (?, ?, ?, ?)",
        vec![
            Value::from(COMPANY_ID),
            Value::from("country_pack"),
            Value::from("de"),
            Value::from(NOW),
        ],
    )
    .await;
    for period in periods {
        exec(
            db,
            "INSERT INTO accounting_period (company_id, period, status) VALUES (?, ?, ?)",
            vec![
                Value::from(COMPANY_ID),
                Value::from(*period),
                Value::from("open"),
            ],
        )
        .await;
    }
    seed_accounts(db).await;
    exec(
        db,
        "INSERT INTO journal (company_id, id, code, kind, name, created_at) VALUES (?, ?, ?, ?, ?, ?)",
        vec![
            Value::from(COMPANY_ID),
            Value::from("GN"),
            Value::from("GN"),
            Value::from("general"),
            Value::from("General Journal"),
            Value::from(NOW),
        ],
    )
    .await;
    for (id, name, vat_id, role) in &[
        ("cust_acme", "Acme GmbH", "DE987654321", "customer"),
        ("cust_fjordlys", "Fjordlys AB", "SE556614220001", "customer"),
        ("vendor_supplier", "Some Supplier", "DE111111111", "vendor"),
    ] {
        exec(
            db,
            "INSERT INTO vendor (id, company_id, name, vat_id, iban, source, roles, needs_review, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, 0, ?, ?)",
            vec![
                Value::from(*id),
                Value::from(COMPANY_ID),
                Value::from(*name),
                Value::from(*vat_id),
                Value::from(None::<String>),
                Value::from("manual"),
                Value::from(*role),
                Value::from(NOW),
                Value::from(NOW),
            ],
        )
        .await;
    }
    exec(
        db,
        "INSERT INTO fiscal_year (company_id, label, start_month, has_adjustment_period, starts_on, ends_on, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        vec![
            Value::from(COMPANY_ID),
            Value::from(2026i64),
            Value::from(1i64),
            Value::from(0i64),
            Value::from("2026-01-01"),
            Value::from("2026-12-31"),
            Value::from(NOW),
            Value::from(NOW),
        ],
    )
    .await;
}

#[allow(dead_code)]
pub async fn seed_company(db: &dyn Database) {
    seed_company_base(db, &["2026-10"]).await;
}

#[allow(dead_code)]
pub async fn seed_company_with_periods(db: &dyn Database) {
    seed_company_base(db, &["2026-01", "2026-03", "2026-10"]).await;
}

pub async fn sum_journal_line_base(db: &dyn Database, entry_id: &str) -> (i64, i64) {
    let rows = db
        .query(&Statement::with_values(
            "SELECT SUM(CASE WHEN base_minor > 0 THEN base_minor ELSE 0 END) as debit, \
             SUM(CASE WHEN base_minor < 0 THEN -base_minor ELSE 0 END) as credit FROM journal_line WHERE entry_id = ?"
                .to_string(),
            vec![Value::from(entry_id)],
        ))
        .await
        .unwrap();
    let row = &rows.rows[0];
    (
        row.get("debit").unwrap_or(0),
        row.get("credit").unwrap_or(0),
    )
}

#[allow(dead_code)]
pub async fn get_journal_lines(db: &dyn Database, entry_id: &str) -> Vec<(String, String, i64)> {
    let rows = db
        .query(&Statement::with_values(
            "SELECT account_number, base_minor FROM journal_line WHERE entry_id = ? ORDER BY account_number"
                .to_string(),
            vec![Value::from(entry_id)],
        ))
        .await
        .unwrap();

    let mut lines = Vec::new();
    for row in rows.rows {
        let account: String = row.get("account_number").unwrap_or_default();
        let base_minor: i64 = row.get("base_minor").unwrap_or(0);
        let side = if base_minor > 0 { "debit" } else { "credit" };
        let amount = base_minor.abs();
        lines.push((account, side.to_string(), amount));
    }
    lines
}
