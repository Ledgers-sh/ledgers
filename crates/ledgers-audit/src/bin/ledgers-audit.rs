//! `ledgers-audit verify <export.jsonl | ->` — the offline verifier as
//! a command.
//!
//! This is what an auditor, a customer's bookkeeper or the company
//! itself runs on an export file: it answers "is this log intact?" with
//! nothing but the file, no database and no network. It becomes
//! `ledgers audit verify` when the `ledgers-cli` crate lands
//! (docs/PLAN.md §6), which is where argument parsing goes once there is
//! a command registry to generate from; until then the arguments are
//! hand-parsed rather than pulling in a parser dependency.
//!
//! The exit codes are the part worth stating: 0 the chain verified, 1
//! the chain is broken (the break goes to stderr), 2 the caller got the
//! usage wrong. A script can branch on 1 without parsing text.

use ledgers_audit::verify;
use std::io::Read as _;
use std::process::ExitCode;

const USAGE: &str = "usage: ledgers-audit verify <export.jsonl | ->";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [command, path] = args.as_slice() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    if command != "verify" {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }

    let bytes: Result<Vec<u8>, String> = if path == "-" {
        let mut bytes = Vec::new();
        std::io::stdin()
            .read_to_end(&mut bytes)
            .map(|_| bytes)
            .map_err(|err| format!("cannot read stdin: {err}"))
    } else {
        std::fs::read(path).map_err(|err| format!("cannot read {path}: {err}"))
    };

    // A read failure and a break are different answers — "I could not
    // look" is not "what I looked at is false" — and both leave through
    // the same arm here, with the message saying which.
    match bytes.and_then(|b| verify(&b).map_err(|k| k.to_string())) {
        Ok(verified) => {
            println!(
                "ok: {} events, head seq {} {}",
                verified.events, verified.head_seq, verified.head_hash
            );
            ExitCode::from(0)
        }
        Err(message) => {
            eprintln!("broken: {message}");
            ExitCode::from(1)
        }
    }
}
