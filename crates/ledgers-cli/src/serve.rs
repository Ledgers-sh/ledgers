//! `ledgers serve`: runs the native server in this process.
//!
//! The server composition is the one `ledgers-server` exposes: [`ledgers_server::harness`]
//! builds the harness from the command registry, and the native runtime serves it.
//! The configuration comes from the environment, as `ledgers-server`'s binary reads it:
//! `DATABASE_PATH` (default `ledgers.db`), `BLOB_DIR` (default `./blobs`), and
//! `LISTEN_ADDR` (read by the native runtime, default `127.0.0.1:3000`).

use crate::error::{CliError, Result};
use cratefield_adapter_sqlite::SqliteDatabase;
use cratefield_core::Config;
use cratefield_runtime_native::{DirBlob, Native, serve as run_native};
use ledgers_commands::Registry;
use std::env;
use std::sync::Arc;

/// Reads configuration from the process environment.
struct EnvConfig;

impl Config for EnvConfig {
    fn get(&self, key: &str) -> Option<String> {
        env::var(key).ok()
    }
}

/// Starts the native server and serves until it stops.
///
/// # Errors
/// Returns an error if the database cannot be opened, the harness cannot be
/// built from the environment, or the listener fails.
pub async fn serve(registry: &Registry) -> Result<()> {
    let database_path = env::var("DATABASE_PATH").unwrap_or_else(|_| "ledgers.db".to_string());
    let blob_dir = env::var("BLOB_DIR").unwrap_or_else(|_| "./blobs".to_string());

    let db = SqliteDatabase::open(&database_path)
        .map_err(|e| CliError::Serve(format!("cannot open database {database_path}: {e}")))?;
    let native = Native::new().db(db).blob_arc(DirBlob::arc(&blob_dir));

    let harness = ledgers_server::harness(registry, &EnvConfig, native.clone())
        .map_err(|e| CliError::Serve(format!("invalid configuration: {e}")))?;

    run_native(Arc::new(harness), native)
        .await
        .map_err(|e| CliError::Serve(e.to_string()))
}
