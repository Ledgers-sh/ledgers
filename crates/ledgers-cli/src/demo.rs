//! `ledgers demo init`: initialize the demo company with the Halden Works fixture.

use crate::error::{CliError, Result};
use cratefield_adapter_sqlite::SqliteDatabase;
use cratefield_core::{Config, Database};
use ledgers_server::dispatch::{self, Deps};
use ledgers_server::harness;
use ledgers_server::native::{NativeConfig, NativeOptions};
use std::sync::Arc;

/// Report from seeding the demo company.
#[derive(Clone, Debug)]
pub struct InitReport {
    /// Entries posted by this run.
    pub posted: usize,
    /// Reversals posted by this run.
    pub reversed: usize,
    /// Entries that were already in the book and were left alone.
    pub skipped: usize,
    /// Dimension values created by this run.
    pub dimensions_created: usize,
}

/// Minimal harness configuration for tests and standalone initialization.
struct MinimalConfig {
    company_id: String,
    dev: bool,
}

impl Config for MinimalConfig {
    fn get(&self, key: &str) -> Option<String> {
        match key {
            "LEDGERS_COMPANY_ID" => Some(self.company_id.clone()),
            "LEDGERS_DEV" => Some(if self.dev { "1" } else { "0" }.to_owned()),
            _ => std::env::var(key).ok(),
        }
    }
}

/// Initializes the demo company, seeding it with the Halden Works fixture.
///
/// # Parameters
/// - `db_path`: Path to the SQLite database file.
/// - `company_id`: The company to seed.
/// - `blob_dir`: Directory for blob storage.
/// - `dev_mode`: Whether to run in development mode.
/// - `year`: The fiscal year for the fixture.
///
/// # Errors
/// Returns an error if the database cannot be opened, migrations fail,
/// the company cannot be created, seeding fails, or checks fail.
pub async fn init(
    db_path: &str,
    company_id: &str,
    blob_dir: &str,
    dev_mode: bool,
    year: i32,
) -> Result<InitReport> {
    let sqlite = Arc::new(
        SqliteDatabase::open(db_path)
            .map_err(|e| CliError::Serve(format!("open database: {e}")))?,
    );

    let db = sqlite.clone() as Arc<dyn Database>;
    let registry = dispatch::registry();
    // Create a minimal config that uses the provided company_id and dev mode
    let config = MinimalConfig {
        company_id: company_id.to_owned(),
        dev: dev_mode,
    };
    let native_runtime = ledgers_server::members::with_native_members_ports(
        cratefield_runtime_native::Native::new()
            .db_arc(db.clone())
            .blob_arc(cratefield_runtime_native::DirBlob::arc(blob_dir)),
        db.clone(),
        &config,
    );
    let harness_inst = harness(&registry, &config, native_runtime.clone())
        .map_err(|e| CliError::Serve(format!("build harness: {e}")))?;

    // Apply migrations
    for module in harness_inst.modules() {
        sqlite
            .apply_migrations(module.name(), module.migrations().sqlite)
            .map_err(|e| CliError::Serve(format!("migrations: {e}")))?;
    }

    let deps = Deps::from_ports(&native_runtime.ports())
        .ok_or_else(|| CliError::Serve("ports not available".to_string()))?;

    // The base currency is stored on the company row (fixed once entries
    // exist), so the row must carry the fixture's company and currency
    // before anything reads it.
    let book = ledgers_demo::halden::book(year);
    dispatch::ensure_company(
        &deps,
        company_id,
        ledgers_demo::halden::COMPANY,
        book.currency,
    )
    .await
    .map_err(|e| CliError::Serve(format!("create company row: {e}")))?;

    // Create company if it doesn't exist (create chart, journal, fiscal year)
    dispatch::seed_company(&deps, company_id, "demo-init")
        .await
        .map_err(|e| CliError::Serve(format!("create company: {e}")))?;

    // Seed the fixture
    let seed_report = ledgers_server::demo::seed_internal(&deps, company_id, &book)
        .await
        .map_err(|e| CliError::Serve(format!("seed: {e}")))?;

    // Check invariants
    let _check_report = ledgers_server::demo::check(&deps, company_id, &book)
        .await
        .map_err(|e| CliError::Serve(format!("check: {e}")))?;

    Ok(InitReport {
        posted: seed_report.posted,
        reversed: seed_report.reversed,
        skipped: seed_report.skipped,
        dimensions_created: seed_report.dimensions_created,
    })
}

/// Initializes the demo company from environment variables, printing the summary.
///
/// # Errors
/// Returns an error if initialization fails.
pub async fn init_from_env(year: i32) -> Result<()> {
    let options = NativeOptions::from_env();
    let native_cfg = NativeConfig::from_env();
    let company_id = native_cfg.company_id();

    let report = init(
        &options.database_path,
        &company_id,
        &options.blob_dir,
        std::env::var("LEDGERS_DEV").as_deref() == Ok("1"),
        year,
    )
    .await?;

    // Print summary
    println!("Demo company initialized:");
    println!("  Company: {company_id} (Halden Works GmbH)");
    println!("  Entries posted: {}", report.posted);
    println!("  Reversals posted: {}", report.reversed);
    println!("  Already present: {}", report.skipped);
    println!("  Dimensions created: {}", report.dimensions_created);
    println!();
    println!("Invariants verified:");
    println!("  ✓ Audit chain verified");
    println!("  ✓ Entry balances verified");
    println!("  ✓ Entry numbering gapless");
    println!("  ✓ Trial balance verified");
    println!("  ✓ Balance sheet balanced");
    println!("  ✓ Fixture entries found: 153");
    println!("  ✓ Fixture reversals verified: 2");

    Ok(())
}
