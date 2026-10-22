//! The native (SQLite) composition: the entry point `ledgers-server` and
//! `ledgers serve` both run. It opens the database, builds the harness,
//! applies every module's migrations, runs the development bootstrap when
//! `LEDGERS_DEV=1`, and serves until shutdown.

use crate::dispatch::{self, Deps};
use crate::{harness, is_dev};
use cratefield_adapter_sqlite::SqliteDatabase;
use cratefield_core::{Config, Database};
use cratefield_runtime_native::{DirBlob, Native, serve};
use std::env;
use std::sync::Arc;

/// Where the native server keeps its state.
#[derive(Clone, Debug)]
pub struct NativeOptions {
    /// The SQLite database file (`DATABASE_PATH`, default `ledgers.db`).
    pub database_path: String,
    /// The directory for document blobs (`BLOB_DIR`, default `./blobs`).
    pub blob_dir: String,
}

impl NativeOptions {
    /// Reads `DATABASE_PATH` and `BLOB_DIR` from the process environment.
    #[must_use]
    pub fn from_env() -> Self {
        NativeOptions {
            database_path: env::var("DATABASE_PATH").unwrap_or_else(|_| "ledgers.db".to_string()),
            blob_dir: env::var("BLOB_DIR").unwrap_or_else(|_| "./blobs".to_string()),
        }
    }
}

/// Runs the native server until shutdown. In development mode
/// (`LEDGERS_DEV=1`) it generates a throwaway `HARNESS_SECRET` when none is
/// set, runs the dev bootstrap, and prints the dev key once to stdout.
///
/// # Errors
/// Database, configuration, migration, bootstrap or bind failures.
pub async fn run(options: NativeOptions) -> Result<(), Box<dyn std::error::Error>> {
    let cfg = NativeConfig::from_env();
    let dev = is_dev(&cfg);

    let sqlite = Arc::new(SqliteDatabase::open(&options.database_path)?);
    let native = Native::new()
        .db_arc(sqlite.clone() as Arc<dyn Database>)
        .blob_arc(DirBlob::arc(&options.blob_dir));

    let registry = dispatch::registry();
    let harness_inst = harness(&registry, &cfg, native.clone())?;

    for module in harness_inst.modules() {
        sqlite.apply_migrations(module.name(), module.migrations().sqlite)?;
    }
    tracing::info!("Using SQLite database at {}", options.database_path);

    if dev {
        let deps = Deps::from_ports(&native.ports())
            .ok_or("the native runtime provides the db, clock and id generator")?;
        let token = dispatch::bootstrap_dev(&deps, &cfg.company_id())
            .await
            .map_err(|err| format!("dev bootstrap failed: {err}"))?;
        println!("dev key: {token}");
    }

    tracing::info!("Starting Ledgers server");
    serve(Arc::new(harness_inst), native).await?;
    Ok(())
}

/// Configuration read from the process environment. In development mode a
/// missing `HARNESS_SECRET` is replaced by a throwaway one, with a warning.
pub struct NativeConfig {
    dev: bool,
    generated_secret: Option<String>,
}

impl NativeConfig {
    /// Reads the environment and decides whether to generate a secret.
    ///
    /// # Panics
    /// When the operating system has no entropy source, which a server cannot
    /// run without in development mode.
    #[must_use]
    pub fn from_env() -> Self {
        let dev = env::var("LEDGERS_DEV").as_deref() == Ok("1");
        let generated_secret = if dev && env::var("HARNESS_SECRET").is_err() {
            let mut bytes = [0u8; 32];
            getrandom::fill(&mut bytes).expect("the operating system entropy source is available");
            let mut secret = String::with_capacity(bytes.len() * 2);
            for byte in bytes {
                let _ = std::fmt::Write::write_fmt(&mut secret, format_args!("{byte:02x}"));
            }
            eprintln!(
                "warning: HARNESS_SECRET is unset; generated a throwaway secret for LEDGERS_DEV=1 (not for production)"
            );
            Some(secret)
        } else {
            None
        };
        NativeConfig {
            dev,
            generated_secret,
        }
    }

    /// The served company: `LEDGERS_COMPANY_ID`, or the dev fixture in dev mode.
    #[must_use]
    pub fn company_id(&self) -> String {
        env::var("LEDGERS_COMPANY_ID").unwrap_or_else(|_| {
            if self.dev {
                dispatch::DEV_COMPANY_ID.to_string()
            } else {
                String::new()
            }
        })
    }
}

impl Config for NativeConfig {
    fn get(&self, key: &str) -> Option<String> {
        if key == "HARNESS_SECRET"
            && let Some(secret) = &self.generated_secret
        {
            return Some(secret.clone());
        }
        env::var(key).ok()
    }
}
