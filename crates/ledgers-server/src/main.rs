#![forbid(unsafe_code)]
//! Ledgers server: native binary entry point.
//!
//! Runs the Ledgers harness on the native runtime with SQLite or Postgres,
//! depending on the DATABASE_URL environment variable.

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use cratefield_adapter_sqlite::SqliteDatabase;
    use cratefield_core::Config;
    use cratefield_runtime_native::{DirBlob, Native, serve};
    use ledgers_commands::Registry;
    use ledgers_server::harness;
    use std::env;
    use std::sync::Arc;

    pub async fn main() -> Result<(), Box<dyn std::error::Error>> {
        // Environment configuration
        let database_path = env::var("DATABASE_PATH").unwrap_or_else(|_| "ledgers.db".to_string());
        let blob_dir = env::var("BLOB_DIR").unwrap_or_else(|_| "./blobs".to_string());

        // Create the harness configuration
        let cfg = NativeConfig::new();

        // Set up the native runtime with SQLite database and blob storage
        let db = setup_database(&database_path)?;
        let blob = DirBlob::arc(&blob_dir);
        let native = Native::new().db(db).blob_arc(blob);

        // Build the harness with the runtime (so it knows which ports are provided)
        let registry = Registry::new();
        let harness_inst = harness(&registry, &cfg, native.clone())?;

        tracing::info!("Starting Ledgers server");

        // Run the server (reads LISTEN_ADDR from environment, handles migrations)
        serve(Arc::new(harness_inst), native).await?;

        Ok(())
    }

    /// Minimal configuration struct for native runtime
    pub struct NativeConfig;

    impl NativeConfig {
        fn new() -> Self {
            NativeConfig
        }
    }

    impl Config for NativeConfig {
        fn get(&self, key: &str) -> Option<String> {
            env::var(key).ok()
        }
    }

    /// Set up the database: SQLite by default
    fn setup_database(sqlite_path: &str) -> Result<SqliteDatabase, Box<dyn std::error::Error>> {
        let db = SqliteDatabase::open(sqlite_path)?;
        tracing::info!("Using SQLite database at {}", sqlite_path);
        Ok(db)
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    native::main().await
}

#[cfg(target_arch = "wasm32")]
fn main() {}
