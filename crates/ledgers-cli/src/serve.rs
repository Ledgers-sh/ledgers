//! `ledgers serve`: runs the native server in this process.
//!
//! The composition is `ledgers_server::native::run`, the same one the
//! `ledgers-server` binary runs. It reads `DATABASE_PATH` (default
//! `ledgers.db`), `BLOB_DIR` (default `./blobs`), and `LISTEN_ADDR` (read by the
//! native runtime, default `127.0.0.1:3000`), and the development switches
//! `LEDGERS_DEV` and `HARNESS_SECRET`.

use crate::error::{CliError, Result};
use ledgers_server::native::{NativeOptions, run};

/// Starts the native server and serves until it stops.
///
/// # Errors
/// Returns an error if the database cannot be opened, the configuration is
/// invalid, the development bootstrap fails, or the listener fails.
pub async fn serve() -> Result<()> {
    run(NativeOptions::from_env())
        .await
        .map_err(|e| CliError::Serve(e.to_string()))
}
