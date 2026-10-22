#![forbid(unsafe_code)]
//! Ledgers CLI: the `ledgers` binary.
//!
//! - `ledgers <resource> [<sub>] <verb>`: every command in the registry, sent over
//!   HTTP to a Ledgers server (see [`app`] and [`request`]).
//! - `ledgers serve`: runs the native server in this process ([`serve`]).
//! - `ledgers mcp`: relays MCP over stdio to the server's `/v1/mcp` ([`mcp`]).
//! - `ledgers completions <shell>`: shell completions.
//!
//! # Configuration
//!
//! Profiles live in `$LEDGERS_CONFIG_DIR/config.toml` or `~/.config/ledgers/config.toml`
//! (see [`profile`]). Precedence:
//!
//! - URL: `--url`, then `LEDGERS_URL`, then the profile's `url`.
//! - Key: `LEDGERS_API_KEY`, then the profile's key reference (`keychain` for the OS
//!   keychain, or `env:VAR`). Literal keys are rejected in the config file.
//! - Profile name: `--profile`, then `LEDGERS_PROFILE`, then `default`.
//!
//! # Exit codes
//!
//! - `0`: success (or a dry run)
//! - `1`: any error
//! - `2`: needs approval; the approval URL is printed
//! - `3`: denied by policy (HTTP 403)

pub mod app;
pub mod error;
pub mod http;
pub mod mcp;
pub mod output;
pub mod profile;
pub mod request;
pub mod serve;

use app::GlobalFlags;
use clap::ArgMatches;
use ledgers_commands::Registry;
use std::io::Write;
use std::path::PathBuf;

pub use app::cli_registry;
pub use error::{CliError, ExitCode, Result};
pub use http::{CallOutcome, Session};

/// Settings read from the environment, separated out so tests can supply them.
#[derive(Clone, Debug, Default)]
pub struct Context {
    /// The config file to read; `None` means the default location.
    pub config_path: Option<PathBuf>,
    /// `LEDGERS_URL`.
    pub url: Option<String>,
    /// `LEDGERS_API_KEY`.
    pub api_key: Option<String>,
    /// `LEDGERS_PROFILE`.
    pub profile: Option<String>,
}

impl Context {
    /// Reads the context from the process environment.
    #[must_use]
    pub fn from_env() -> Self {
        fn non_empty(name: &str) -> Option<String> {
            std::env::var(name).ok().filter(|v| !v.is_empty())
        }
        Context {
            config_path: non_empty("LEDGERS_CONFIG_DIR")
                .map(|dir| PathBuf::from(dir).join("config.toml")),
            url: non_empty("LEDGERS_URL"),
            api_key: non_empty("LEDGERS_API_KEY"),
            profile: non_empty("LEDGERS_PROFILE"),
        }
    }
}

/// Runs the CLI with the given arguments (including the program name) and returns
/// the exit code. Output goes to `out` and `err`.
pub async fn run(
    args: Vec<String>,
    ctx: &Context,
    registry: &Registry,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> ExitCode {
    let root = app::build_root(registry);
    let matches = match root.try_get_matches_from(args) {
        Ok(matches) => matches,
        Err(e) => {
            let text = e.render().to_string();
            return if e.use_stderr() {
                let _ = err.write_all(text.as_bytes());
                ExitCode::Error
            } else {
                // --help and --version
                let _ = out.write_all(text.as_bytes());
                ExitCode::Ok
            };
        }
    };

    let globals = GlobalFlags::from_matches(&matches);
    match dispatch(&matches, &globals, ctx, registry, out, err).await {
        Ok(code) => code,
        Err(error) => {
            let _ = output::write_error(&error, globals.json, out, err);
            error.exit_code()
        }
    }
}

async fn dispatch(
    matches: &ArgMatches,
    globals: &GlobalFlags,
    ctx: &Context,
    registry: &Registry,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<ExitCode> {
    match matches.subcommand() {
        Some(("serve", _)) => {
            serve::serve().await?;
            Ok(ExitCode::Ok)
        }
        Some(("completions", sub)) => {
            let shell = sub
                .get_one::<String>("shell")
                .ok_or_else(|| CliError::InvalidInput("shell argument required".into()))?;
            generate_completions(registry, shell, out)?;
            Ok(ExitCode::Ok)
        }
        Some(("mcp", sub)) => {
            let connection = profile::resolve(globals, ctx)?;
            let session = Session::new(connection.url, connection.key)?;
            mcp::run_mcp(session, sub.get_flag("read_only")).await?;
            Ok(ExitCode::Ok)
        }
        Some(_) => run_registry_command(matches, globals, ctx, registry, out, err).await,
        None => Err(CliError::InvalidInput("no command given".into())),
    }
}

async fn run_registry_command(
    matches: &ArgMatches,
    globals: &GlobalFlags,
    ctx: &Context,
    registry: &Registry,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<ExitCode> {
    let (path, leaf) = app::leaf_path(matches);
    let descriptor = app::find_descriptor(registry, &path).ok_or_else(|| {
        CliError::Internal(format!("no registry command for `{}`", path.join(" ")))
    })?;

    let invocation = app::invocation(descriptor, leaf)?;
    let plan = request::plan(descriptor, &invocation)?;
    let connection = profile::resolve(globals, ctx)?;
    let session = Session::new(connection.url, connection.key)?;
    let outcome = session.execute(&plan, invocation.dry_run).await?;

    output::write_outcome(&outcome, globals.json, out, err)
        .map_err(|e| CliError::Internal(format!("cannot write output: {e}")))?;

    Ok(match outcome {
        CallOutcome::Done(_) | CallOutcome::DryRun(_) => ExitCode::Ok,
        CallOutcome::NeedsApproval { .. } => ExitCode::NeedsApproval,
        CallOutcome::Denied(_) => ExitCode::PolicyDenied,
    })
}

fn generate_completions(registry: &Registry, shell: &str, out: &mut dyn Write) -> Result<()> {
    let shell = match shell {
        "bash" => clap_complete::Shell::Bash,
        "zsh" => clap_complete::Shell::Zsh,
        "fish" => clap_complete::Shell::Fish,
        "powershell" => clap_complete::Shell::PowerShell,
        other => {
            return Err(CliError::InvalidInput(format!(
                "unsupported shell: {other}"
            )));
        }
    };
    let mut cmd = app::build_root(registry);
    clap_complete::generate(shell, &mut cmd, "ledgers", out);
    Ok(())
}
