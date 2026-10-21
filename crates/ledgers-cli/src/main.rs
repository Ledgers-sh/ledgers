use ledgers_cli::{CliError, app, error::ExitCode, serve};
use ledgers_commands::Registry;
use std::process;

#[tokio::main]
async fn main() {
    let exit_code = run().await;
    process::exit(exit_code as i32);
}

async fn run() -> ExitCode {
    match run_command().await {
        Ok(exit_code) => exit_code,
        Err(e) => {
            eprintln!("error: {}", e);
            e.exit_code()
        }
    }
}

async fn run_command() -> Result<ExitCode, CliError> {
    // Build the registry
    let registry = Registry::new();

    // Build the CLI
    let root = app::build_root(&registry);
    let matches = root.try_get_matches();

    let matches = matches.map_err(|e| CliError::CommandArg(e.to_string()))?;

    // Parse global flags
    let globals = app::GlobalFlags::from_matches(&matches);

    // Handle top-level subcommands
    match matches.subcommand() {
        Some(("serve", _sub_matches)) => {
            serve::serve().await?;
            Ok(ExitCode::Ok)
        }
        Some(("completions", sub_matches)) => {
            let shell = sub_matches
                .get_one::<String>("shell")
                .ok_or_else(|| CliError::CommandArg("shell argument required".to_string()))?;

            generate_completions(&registry, shell)?;
            Ok(ExitCode::Ok)
        }
        Some((cmd_name, sub_matches)) => {
            // This is a registry command; dispatch it
            dispatch_registry_command(&registry, &globals, cmd_name, sub_matches).await
        }
        None => Err(CliError::CommandArg("no subcommand specified".to_string())),
    }
}

#[allow(unused)]
async fn dispatch_registry_command(
    registry: &Registry,
    globals: &app::GlobalFlags,
    cmd_path: &str,
    matches: &clap::ArgMatches,
) -> Result<ExitCode, CliError> {
    // Reconstruct the full command name from the path
    // This is a simplified version; a real implementation would need to
    // navigate nested subcommands properly
    let _full_cmd = cmd_path.to_string();

    // For now, return a placeholder error
    Err(CliError::Internal(
        "registry command dispatch not yet implemented".to_string(),
    ))
}

fn generate_completions(registry: &Registry, shell: &str) -> Result<(), CliError> {
    let root = app::build_root(registry);

    let completion_gen = match shell {
        "bash" => clap_complete::Shell::Bash,
        "zsh" => clap_complete::Shell::Zsh,
        "fish" => clap_complete::Shell::Fish,
        "powershell" => clap_complete::Shell::PowerShell,
        _ => return Err(CliError::CommandArg(format!("unsupported shell: {shell}"))),
    };

    let mut stdout = std::io::stdout();
    clap_complete::generate(completion_gen, &mut root.clone(), "ledgers", &mut stdout);

    Ok(())
}
