//! The `ledgers` binary. All logic lives in the library crate.

use ledgers_cli::{Context, cli_registry, run};
use std::io::Write;
use std::process;

#[tokio::main]
async fn main() {
    let registry = match cli_registry() {
        Ok(registry) => registry,
        Err(e) => {
            eprintln!("error: invalid command registry: {e}");
            process::exit(1);
        }
    };
    let args: Vec<String> = std::env::args().collect();
    let ctx = Context::from_env();

    let mut out = std::io::stdout();
    let mut err = std::io::stderr();
    let code = run(args, &ctx, &registry, &mut out, &mut err).await;
    let _ = out.flush();
    let _ = err.flush();
    process::exit(code.code());
}
