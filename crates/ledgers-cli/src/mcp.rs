//! CLI MCP stdio relay: reads JSON-RPC from stdin, POSTs to server, writes response to stdout.

use crate::{CliError, Session};
use reqwest::Client;
use serde_json::{Value, json};
use std::io::{self, BufRead, BufReader, Write};

/// Runs the MCP stdio relay.
///
/// Reads newline-delimited JSON-RPC messages from stdin, POSTs each to the server's
/// `/v1/mcp` endpoint with the agent key as Bearer token, and writes responses to stdout.
pub async fn run_mcp(session: Session, read_only: bool) -> Result<(), CliError> {
    let client = Client::new();
    let mut url = session.url.clone();
    if url.ends_with('/') {
        url.pop();
    }
    url.push_str("/v1/mcp");

    let stdin = io::stdin();
    let reader = BufReader::new(stdin.lock());
    let mut stdout = io::stdout();

    for line_result in reader.lines() {
        let line = line_result.map_err(|e| CliError::Internal(format!("stdin error: {e}")))?;

        // Skip blank lines
        if line.trim().is_empty() {
            continue;
        }

        // Parse JSON
        let message: Value = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(_e) => {
                let error_response = json!({
                    "jsonrpc": "2.0",
                    "error": {
                        "code": -32700,
                        "message": "Parse error"
                    }
                });
                writeln!(stdout, "{}", error_response)
                    .map_err(|e| CliError::Internal(format!("stdout error: {e}")))?;
                stdout
                    .flush()
                    .map_err(|e| CliError::Internal(format!("flush error: {e}")))?;
                continue;
            }
        };

        // POST to server
        let mut request = client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .bearer_auth(&session.key);

        if read_only {
            request = request.header("ledgers-read-only", "true");
        }

        let response = match request.body(line).send().await {
            Ok(resp) => resp,
            Err(e) => {
                // HTTP error: write a JSON-RPC error
                let is_request_with_id = message.get("id").is_some();

                if is_request_with_id {
                    let error_response = json!({
                        "jsonrpc": "2.0",
                        "id": message.get("id"),
                        "error": {
                            "code": -32603,
                            "message": format!("HTTP error: {}", e)
                        }
                    });
                    writeln!(stdout, "{}", error_response)
                        .map_err(|e| CliError::Internal(format!("stdout error: {e}")))?;
                } else {
                    // Notification: log to stderr only
                    eprintln!("HTTP error: {}", e);
                }

                stdout
                    .flush()
                    .map_err(|e| CliError::Internal(format!("flush error: {e}")))?;
                continue;
            }
        };

        // Write response if it's not a 202 (notification)
        if response.status() != 202
            && let Ok(body) = response.text().await
        {
            writeln!(stdout, "{body}")
                .map_err(|e| CliError::Internal(format!("stdout error: {e}")))?;
        }

        stdout
            .flush()
            .map_err(|e| CliError::Internal(format!("flush error: {e}")))?;
    }

    Ok(())
}
