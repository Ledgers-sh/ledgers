#![forbid(unsafe_code)]
//! Ledgers server: native binary entry point. Runs the composition in
//! `ledgers_server::native::run` (SQLite, `DATABASE_PATH`, `LEDGERS_DEV`).

#[cfg(not(target_arch = "wasm32"))]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ledgers_server::native::run(ledgers_server::native::NativeOptions::from_env()).await
}

#[cfg(target_arch = "wasm32")]
fn main() {}
