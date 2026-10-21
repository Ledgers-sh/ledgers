//! Server startup: `ledgers serve`.

use crate::error::{CliError, Result};
use std::env;

/// Starts the native Ledgers server.
///
/// Respects environment variables: DATABASE_PATH, BLOB_DIR, LISTEN_ADDR.
/// Can be overridden by command-line flags (not implemented yet).
///
/// # Errors
/// Returns an error if the server cannot start.
pub async fn serve() -> Result<()> {
    // Set up tracing
    tracing_subscriber::fmt::init();

    // Environment configuration (can be overridden by future flags)
    let database_path = env::var("DATABASE_PATH").unwrap_or_else(|_| "ledgers.db".to_string());
    let blob_dir = env::var("BLOB_DIR").unwrap_or_else(|_| "./blobs".to_string());
    let listen_addr = env::var("LISTEN_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".to_string());

    tracing::info!(
        "Starting Ledgers server: database={}, blobs={}, listen={}",
        database_path,
        blob_dir,
        listen_addr
    );

    // Call the server startup function (to be created in ledgers-server)
    // For now, this is a placeholder that documents the intent.
    // The follow-up will extract main.rs into a pub fn that we call here.
    // The function will read DATABASE_PATH, BLOB_DIR, LISTEN_ADDR from env.

    Err(CliError::Serve(
        "ledgers serve not yet wired to the server startup function".to_string(),
    ))
}
