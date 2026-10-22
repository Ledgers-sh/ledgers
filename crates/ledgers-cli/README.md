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
export LEDGERS_API_KEY=...   # or a profile key reference, see below
ledgers --url http://localhost:3000 entries list

ledgers --profile production entries reverse EXT-42 --input '{"reason": "typo"}'

ledgers --profile production entries post --input @request.json --dry-run
```

Each command takes its path arguments positionally (`entries reverse EXT-42`),
its whole input as `--input <JSON|@FILE|->`, and one `--<field>` flag per input
field of its schema (`--bill-id B-1`). `--input` and the flags can be combined;
a flag wins over the same key in `--input`. `--help` on any command shows its
HTTP route, tier and the fields it takes.

Global flags:
- `--json` — machine-readable output on stdout for every outcome, including
  approvals, denials and errors (see below)
- `--profile <NAME>` — select a named profile
- `--url <URL>` — server URL (overrides `LEDGERS_URL` and the profile)

`--dry-run` is offered only on commands that support it. It previews the
command and its policy decision and writes nothing.

### Start the server

```bash
ledgers serve
```

Respects environment variables:
- `DATABASE_PATH` — SQLite database (default: `ledgers.db`)
- `BLOB_DIR` — document storage (default: `./blobs`)
- `LISTEN_ADDR` — server address (default: `127.0.0.1:3000`)

### Initialize the demo company

```bash
ledgers demo init
```

Creates the Halden Works GmbH demo company (an invented German small manufacturer) seeded with a full fiscal year of entries that pass every invariant check. Idempotent: running twice posts nothing new and still passes all checks.

Respects environment variables:
- `DATABASE_PATH` — SQLite database (default: `ledgers.db`)
- `LEDGERS_COMPANY_ID` — company id (default: `01DEVCOMPANY0000000000000A`)
- `--year <YEAR>` — fiscal year for the fixture (default: 2026)

### Generate shell completions

```bash
ledgers completions bash | sudo tee /usr/local/etc/bash_completion.d/ledgers
ledgers completions zsh | sudo tee /usr/share/zsh/site-functions/_ledgers
```

## Profiles and Authentication

Configuration file: `$LEDGERS_CONFIG_DIR/config.toml`, or `~/.config/ledgers/config.toml`.

```toml
[profiles.default]
url = "http://localhost:3000"
key = "keychain"  # the OS keychain entry for this profile, or "env:VAR_NAME"

[profiles.production]
url = "https://ledgers.example.com"
key = "env:PROD_LEDGERS_KEY"
```

The server URL comes from `--url`, then `LEDGERS_URL`, then the profile.

The API key comes from `LEDGERS_API_KEY` if it is set. Otherwise it comes from the
profile's key reference:
- `keychain` reads the OS keychain item with service `ledgers` and account = the
  profile name. On macOS, create it with
  `security add-generic-password -s ledgers -a default -w` (use your profile name for `-a`).
- `env:VAR_NAME` reads that environment variable.

A literal key in the config file is rejected when the file is read, and the error
does not print the key. There is no `--key` flag, so a key never appears in shell
history.

Profile selection: `--profile`, then `LEDGERS_PROFILE`, then `default`.

**Security:** keys are never written to the config file.

## Exit Codes

- `0` — Success, or a dry run
- `1` — Any other error (network, server error, invalid input, missing key or URL)
- `2` — Needs approval. The approval URL and ID are printed to stdout (`--json`: the
  `approval` object).
- `3` — Denied by policy (HTTP 403). The reason is printed to stderr (`--json`: stdout).

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

Tests (in `tests/cli_suite`) cover:
- request mapping for every registry command (method, path, query, body)
- exit codes 0, 1, 2 and 3, and `--json` output, against a local mock server
- profiles: key references, literal-key rejection, URL and key precedence
- help-text snapshots for the root, every built-in, every group and every command

## Not yet

- `ledgers login` (device flow) and commands that write keychain entries. Until then,
  store a key with the `security` command above or use `LEDGERS_API_KEY`.
- Validating command output against its output schema.
