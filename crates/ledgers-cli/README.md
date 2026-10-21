# ledgers-cli

The Ledgers command-line interface: execute accounting commands via HTTP, manage server profiles, and start the native server.

## Installation

```bash
cargo install ledgers-cli
```

## Usage

### Execute a command

Every command in the registry is available as a CLI subcommand. For example:

```bash
ledgers --url http://localhost:3000 --key $LEDGERS_KEY bills match \
  --input '{"bill_id": "123", ...}'

ledgers --profile production bills match \
  --input @request.json

ledgers --profile prod bills match \
  --dry-run --input @request.json
```

Global flags:
- `--json` — output machine JSON (default: human-readable)
- `--profile <NAME>` — select a named profile
- `--url <URL>` — server URL (overrides profile)
- `--key <KEY>` — API key (overrides profile)

### Start the server

```bash
ledgers serve
```

Respects environment variables:
- `DATABASE_PATH` — SQLite database (default: `ledgers.db`)
- `BLOB_DIR` — document storage (default: `./blobs`)
- `LISTEN_ADDR` — server address (default: `127.0.0.1:3000`)

### Generate shell completions

```bash
ledgers completions bash | sudo tee /usr/local/etc/bash_completion.d/ledgers
ledgers completions zsh | sudo tee /usr/share/zsh/site-functions/_ledgers
```

## Profiles and Authentication

Configuration file: `$LEDGERS_CONFIG_DIR/config.toml` or `$XDG_CONFIG_HOME/ledgers/config.toml` or `~/.config/ledgers/config.toml`.

```toml
[profiles.default]
url = "http://localhost:3000"
key = "keychain"  # stored in OS keychain, or use "env:VAR_NAME"

[profiles.production]
url = "https://ledgers.example.com"
key = "env:PROD_LEDGERS_KEY"
```

Profile selection priority:
1. `--profile` flag
2. `LEDGERS_PROFILE` environment variable
3. `default` profile in config

Environment variable overrides:
- `LEDGERS_URL` — server URL (literal, not a reference)
- `LEDGERS_KEY` — API key (literal, not a reference)

**Security:** Never store literal API keys in the config file. Use the OS keychain (`keychain`) or environment variables (`env:VAR_NAME`).

## Exit Codes

- `0` — Success
- `1` — Error (network, server 5xx, invalid input, authentication, etc.)
- `2` — Needs approval (approval URL printed to stdout)
- `3` — Denied by policy

## Architecture

The CLI is built on the [Ledgers commands registry](../ledgers-commands), which defines the HTTP interface. Every command is mapped to:
- **HTTP route** (e.g., `POST /v1/bills/{id}/match`)
- **CLI form** (e.g., `ledgers bills match`)
- **Input/output schemas** (JSON Schema)

The registry is compiled into the server and the CLI; they speak the same contract.

## HTTP Contract

### Success (2xx)

- **200 OK** — Command executed or dry-run completed
- Response body is JSON matching the command's output schema

### Approval

- **200 OK with special JSON** — Command needs approval
- The response contains approval ID and URL
- CLI exit code: `2`

### Policy Denial

- **403 Forbidden** — Policy denied the command
- CLI exit code: `3`

### Errors (4xx, 5xx)

- Problem responses per RFC 9457 (application/problem+json)
- CLI exit code: `1`

## Testing

Run the test suite:

```bash
cargo test -p ledgers-cli
```

Tests include:
- Help text snapshots (to detect breaking changes)
- Exit code verification
- Output schema validation

## Future Work

- `ledgers login` — device flow authentication
- `ledgers mcp` — run as an MCP server for agent integration
