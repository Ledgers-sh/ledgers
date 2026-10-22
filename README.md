<p align="center">
  <img src="docs/assets/readme-banner.png" alt="ledgers.sh. Agents do the books. People keep control." width="100%">
</p>

<p align="center">
  <a href="https://github.com/Ledgers-sh/ledgers/actions/workflows/ci.yml"><img src="https://github.com/Ledgers-sh/ledgers/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
  <img src="https://img.shields.io/badge/status-pre--release-7FA6C9?style=flat-square&labelColor=0F1217" alt="Status: pre-release, nothing published">
  <img src="https://img.shields.io/badge/rust-1.98.1-E4E7EB?style=flat-square&labelColor=0F1217" alt="Rust 1.98.1">
  <img src="https://img.shields.io/badge/edition-2024-E4E7EB?style=flat-square&labelColor=0F1217" alt="Edition 2024">
  <img src="https://img.shields.io/badge/license-MIT-E4E7EB?style=flat-square&labelColor=0F1217" alt="License: MIT">
  <img src="https://img.shields.io/badge/MCP-adapter%20merged-7FA6C9?style=flat-square&labelColor=0F1217" alt="MCP: adapter merged, not yet published">
  <a href="https://ledgers.sh"><img src="https://img.shields.io/badge/website-ledgers.sh-E4E7EB?style=flat-square&labelColor=0F1217" alt="Website: ledgers.sh"></a>
  <a href="https://cratefield.com"><img src="https://img.shields.io/badge/built%20with-Cratefield-E4E7EB?style=flat-square&labelColor=0F1217" alt="Built with Cratefield"></a>
  <a href="https://factory0.ventures"><img src="https://img.shields.io/badge/a-Factory%20Zero%20venture-E4E7EB?style=flat-square&labelColor=0F1217" alt="A Factory Zero venture"></a>
</p>

<p align="center">
  <b>Ledgers</b>: open-source double-entry accounting that AI agents can work in, with a policy that decides what they may post and a person who approves the rest.
</p>

Agents read invoices, match payments and draft journal entries. A policy decides what each
agent key may do. Anything high-risk waits for a person to approve it, with a passkey. Every
action is audited and reversible. The CLI, the MCP tools and the HTTP API run the same
commands.

**Status: pre-release.** The core crates build and are tested in CI, but nothing is published
to crates.io, no release binary exists and there is no hosted service yet. Do not run
`cargo install ledgers`: that name on crates.io belongs to an unrelated 2020 project.

## Why

Bookkeeping is mostly reading documents and matching them: a vendor bill against a purchase
order, a payment against an invoice. Agents can do that work. They should not post anything
they like, and a single company should be able to keep correct books without trusting a prompt.

Ledgers enforces this in code ([docs/PLAN.md](docs/PLAN.md), section 3):

- **Money is never a float.** Amounts are integer minor units, computed in `i128`, with the
  currency's own exponent.
- **Every entry balances.** The domain type checks it, and a guard statement checks it again
  in the same write.
- **Posted entries are immutable.** A correction is a reversal plus a new entry.
- **Closed periods reject postings.** Reopening is a human-only action.
- **Numbering is gapless** per journal and fiscal year.
- **Every action is audited** in a hash-chained log that an offline verifier can check.
- **Policy runs on the server** for every command. An agent's prompt never decides what it may do.
- **Document text is data, never instructions.**

Agent keys carry a tier (`read`, `draft`, `post-within-limits`) and per-currency limits. Paying
money out, closing or reopening a period, filing tax and changing policy are human-only.

## What works today

Derived from merged code on `main` and the issue tracker. Issue numbers link to the
[milestone plan](https://github.com/Ledgers-sh/ledgers/issues).

| Area | Status | Evidence |
| :--- | :--- | :--- |
| Exact money, currencies, rounding | **In code** | `ledgers-money`, [#3](https://github.com/Ledgers-sh/ledgers/issues/3) closed |
| Storage schema and three-database test matrix | **In code** | `ledgers-storage`, [#4](https://github.com/Ledgers-sh/ledgers/issues/4) closed |
| Chart of accounts and country templates | **In code** | `ledgers-ledger`, [#5](https://github.com/Ledgers-sh/ledgers/issues/5) closed |
| Journal entries, reversal, dry run, idempotency | **In code** | `ledgers-ledger`, [#6](https://github.com/Ledgers-sh/ledgers/issues/6), [#43](https://github.com/Ledgers-sh/ledgers/issues/43) closed |
| Fiscal years, periods, close and reopen | **In code** | [#7](https://github.com/Ledgers-sh/ledgers/issues/7) closed |
| Hash-chained audit log and verifier | **In code** | `ledgers-audit`, [#9](https://github.com/Ledgers-sh/ledgers/issues/9) closed |
| Guard statements on atomic batches | **In code** | `ledgers-db`, [ADR 0004](docs/adr/0004-guard-statements.md), [#40](https://github.com/Ledgers-sh/ledgers/issues/40) closed |
| Tax codes and exchange rates | **In code** | `ledgers-tax`, `ledgers-fx`, [#41](https://github.com/Ledgers-sh/ledgers/issues/41), [#42](https://github.com/Ledgers-sh/ledgers/issues/42) closed |
| Command registry (one definition for CLI, MCP, HTTP) | **In code** | `ledgers-commands`, [#10](https://github.com/Ledgers-sh/ledgers/issues/10) closed |
| Agent key policy, tiers and limits | **In code** | `ledgers-policy`, [#11](https://github.com/Ledgers-sh/ledgers/issues/11) closed |
| Authentication for people and agent keys | **In code** | `ledgers-auth`, [#12](https://github.com/Ledgers-sh/ledgers/issues/12) closed |
| Approval requests with journal preview | **In code** | `ledgers-approvals`, [#13](https://github.com/Ledgers-sh/ledgers/issues/13) closed |
| Extension traits with open defaults | **In code** | `ledgers-ext`, [#14](https://github.com/Ledgers-sh/ledgers/issues/14) closed |
| HTTP server (SQLite, native and Worker composition) | **In code** | `ledgers-server`, [#15](https://github.com/Ledgers-sh/ledgers/issues/15) closed |
| MCP adapter (stdio relay and `/v1/mcp`) | **In code** | `ledgers-cli` `mcp`, `ledgers-server` `mcp.rs`, [#17](https://github.com/Ledgers-sh/ledgers/issues/17) closed |
| `ledgers` CLI: registry commands, `serve` | **Not finished** | Registry dispatch and `serve` still return "not yet implemented" in `ledgers-cli` ([#16](https://github.com/Ledgers-sh/ledgers/issues/16) is closed, the dispatch is not) |
| Threat model and prompt-injection defences | **Planned** | [#18](https://github.com/Ledgers-sh/ledgers/issues/18), M1 |
| Approval notifications (webhooks, email, Discord, Slack, push) | **Planned** | [#19](https://github.com/Ledgers-sh/ledgers/issues/19), M1 |
| Documents, extraction, bills, matching | **Planned** | [#20](https://github.com/Ledgers-sh/ledgers/issues/20) to [#23](https://github.com/Ledgers-sh/ledgers/issues/23), M2 |
| Parties, E-invoices in, bills intake | **Planned** | [#44](https://github.com/Ledgers-sh/ledgers/issues/44), [#45](https://github.com/Ledgers-sh/ledgers/issues/45), M2 |
| Invoices, bank import, reconciliation, payments, VAT, reports, export | **Planned** | [#24](https://github.com/Ledgers-sh/ledgers/issues/24) to [#30](https://github.com/Ledgers-sh/ledgers/issues/30), [#47](https://github.com/Ledgers-sh/ledgers/issues/47) to [#50](https://github.com/Ledgers-sh/ledgers/issues/50), M3 |
| The app (PWA, review screen, approvals inbox, capture) | **Planned** | [#31](https://github.com/Ledgers-sh/ledgers/issues/31) to [#34](https://github.com/Ledgers-sh/ledgers/issues/34), [#52](https://github.com/Ledgers-sh/ledgers/issues/52), [#53](https://github.com/Ledgers-sh/ledgers/issues/53), M4 |
| Self-hosting, security review, crates.io publish | **Planned** | [#35](https://github.com/Ledgers-sh/ledgers/issues/35) to [#38](https://github.com/Ledgers-sh/ledgers/issues/38), M5 |

The plan and its milestones (M0 to M5) are in [docs/PLAN.md](docs/PLAN.md), section 7.

## Architecture

One Cargo workspace, resolver 3, edition 2024, MIT. The Cratefield harness is pinned by exact
crates.io versions ([ADR 0001](docs/adr/0001-harness-by-pinned-crates.md)).

| Crate | What it does |
| :--- | :--- |
| `ledgers-money` | Exact money, currency codes and exponents, rounding and allocation |
| `ledgers-db` | Guard-statement helpers on the `Db` port: numbering, periods, limits, audit chain |
| `ledgers-storage` | Storage foundation: schema, migrations, SQLite and Postgres dialects |
| `ledgers-ledger` | Chart of accounts, journals, entries, periods, reversals, close checks |
| `ledgers-audit` | Hash-chained audit log and offline verifier |
| `ledgers-tax` | Tax codes, dated rates as data, line tax calculation |
| `ledgers-fx` | Exchange rate table and lookup |
| `ledgers-commands` | One command registry for CLI, MCP and HTTP |
| `ledgers-policy` | Agent keys, tiers, per-currency limits, allow / needs-approval / deny |
| `ledgers-auth` | Authentication for people and agent keys |
| `ledgers-approvals` | Approval requests with journal preview and step-up |
| `ledgers-ext` | Extension traits, each with an open default |
| `ledgers-server` | Harness composition: HTTP routes, MCP endpoint, native and Worker runtimes (`ledgers-server` binary) |
| `ledgers-cli` | The `ledgers` binary: CLI flags, MCP stdio relay, shell completions |

Templates for country charts and mappings live in [`templates/`](templates/) (for example
`de-hgb.toml`, `generic-sme.toml`). Their data sources are listed in [NOTICE](NOTICE).

## Quick start

You need [rustup](https://rustup.rs). The toolchain in `rust-toolchain.toml` (Rust 1.98.1, with
the `wasm32-unknown-unknown` target) installs itself the first time you run `cargo`.

```sh
git clone https://github.com/Ledgers-sh/ledgers.git
cd ledgers
cargo test --workspace
cargo run -p ledgers-server
```

`ledgers-server` reads three environment variables and defaults to the values shown:

| Variable | Default |
| :--- | :--- |
| `DATABASE_PATH` | `ledgers.db` (SQLite file) |
| `BLOB_DIR` | `./blobs` (document storage) |
| `LISTEN_ADDR` | `127.0.0.1:3000` |

### Try it: a development server and a curl walk-through

With `LEDGERS_DEV=1` the server is for local use only. On start it creates a demo
company with the generic small-business chart and the general journal `GN`, opens the
current year, generates a throwaway `HARNESS_SECRET` if you did not set one (with a
warning), and prints one full-tier API key once:

```sh
LEDGERS_DEV=1 LISTEN_ADDR=127.0.0.1:3000 cargo run -p ledgers-server
# dev key: lk_live_…   (copy it; it is not shown again)
export LEDGERS_KEY=lk_live_…   # the printed key
export LEDGERS_URL=http://127.0.0.1:3000
TODAY=$(date -u +%Y-%m-%d); PERIOD=${TODAY:0:7}
```

The walk-through posts a consulting invoice (5010 debit, 4000 credit, 1000.00 EUR), reads the
trial balance and the P&L, reverses the entry, and reads the trial balance again:

```sh
# 1. Post a balanced entry (external_id is the idempotency key you choose)
curl -X POST "$LEDGERS_URL/v1/entries" -H "Authorization: Bearer $LEDGERS_KEY" \
  -H 'Content-Type: application/json' --data "{\"entry\": {
    \"journal_id\": \"\", \"document_date\": \"$TODAY\", \"posting_date\": \"$TODAY\",
    \"tax_point_date\": null, \"base_currency\": \"EUR\", \"description\": \"Consulting invoice\",
    \"source_documents\": [], \"external_id\": \"quickstart-1\", \"source_app\": null, \"reverses\": null,
    \"lines\": [
      {\"account_number\": \"5010\", \"side\": \"debit\", \"base\": {\"minor\": \"100000\", \"currency\": \"EUR\"}, \"fx\": null, \"tax_code\": null, \"dimensions\": {}, \"memo\": null},
      {\"account_number\": \"4000\", \"side\": \"credit\", \"base\": {\"minor\": \"100000\", \"currency\": \"EUR\"}, \"fx\": null, \"tax_code\": null, \"dimensions\": {}, \"memo\": null}
    ]}}"

# 2. Trial balance for the period (debits and credits per account)
curl "$LEDGERS_URL/v1/reports/trial-balance?period=$PERIOD" -H "Authorization: Bearer $LEDGERS_KEY"

# 3. Profit and loss for the period
curl "$LEDGERS_URL/v1/reports/p-and-l?period=$PERIOD" -H "Authorization: Bearer $LEDGERS_KEY"

# 4. Reverse the entry (posted entries are never edited; this posts the mirror)
curl -X POST "$LEDGERS_URL/v1/entries/quickstart-1/reverse" -H "Authorization: Bearer $LEDGERS_KEY" \
  -H 'Content-Type: application/json' --data '{}'

# 5. The trial balance nets to zero again
curl "$LEDGERS_URL/v1/reports/trial-balance?period=$PERIOD" -H "Authorization: Bearer $LEDGERS_KEY"
```

Without a credential every command answers `401`; a key below the command's tier answers `403`.
The same walk-through runs as a script that exits non-zero on any failed check:
`bash tools/quickstart-smoke.sh`. The HTTP routes are the registry's (`GET /v1/accounts`,
`GET /v1/entries`, `GET /v1/entries/{external_id}`, `POST /v1/entries`,
`POST /v1/entries/{external_id}/reverse`, and the four `GET /v1/reports/…` routes).

The full set of checks CI runs on every pull request is in [CONTRIBUTING.md](CONTRIBUTING.md).

## Run the app

The web app in [`app/`](app/) talks to a local server. Start the server in dev mode, which prints a
one-time key and allows the app's origin through CORS:

```sh
LEDGERS_DEV=1 cargo run -p ledgers-server
# listens on http://127.0.0.1:3000 and prints "dev key: <key>" once
```

Copy the dev key, then in a second terminal:

```sh
cd app
npm install
npm run dev
```

Open http://localhost:5173, keep the server URL at `http://127.0.0.1:3000`, paste the key and
connect. The key is kept in `sessionStorage` for that tab only. Passkey sign-in comes with issue #12.

Checks for the app: `npm run typecheck`, `npm test` (unit tests), `npm run build`, and
`npm run test:e2e` (Playwright against a local fake API, no server needed).

## MCP

The MCP adapter exposes the same registry commands as the CLI and HTTP API, filtered by the
agent key's tier. There is no approve tool for agents; approvals go to a person. The CLI can
relay MCP over stdio to a running server:

```sh
ledgers --url http://127.0.0.1:3000 --key <agent-key> mcp
ledgers --url http://127.0.0.1:3000 --key <agent-key> mcp --read-only
```

The server endpoint is `POST /v1/mcp` with the key as a Bearer token.

## Open core

Ledgers is the complete MIT core for one company: every module above, the policy engine,
approvals, audit, the CLI, the MCP server, the HTTP API and self-hosting.

Paid features that are about more people, more companies or running costs live in a separate
private repository, `Ledgers-sh/ledgers-cloud`, which depends on this one. Examples are
tenancy and billing, roles and approval rules, SSO, live bank feeds, group consolidation and
hosted models. This repository never references it: `tools/check-boundary.sh` fails the build
if code, manifests or workflows do ([ADR 0002](docs/adr/0002-open-core-boundary.md)).

## Security

Please do not open a public issue for a security problem. Report it privately to the maintainers
through GitHub. A `SECURITY.md` file is not published yet. The correctness rules that protect
money and policy are in [docs/PLAN.md](docs/PLAN.md), section 3.

Audit log format: [docs/audit-format.md](docs/audit-format.md). Legal documents:
[docs/legal](docs/legal/).

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) first. There are no bounties on this project. Issues are
planning notes for the team, not paid tasks. Outside pull requests are reviewed only when the
change was agreed in an issue first.

Every pull request must pass `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace`, the wasm check, `cargo deny check` and the three guard scripts.

## Legal and Compliance

> Ledgers is not tax or legal advice. Check rates and returns before you file.

See [docs/legal/DISCLAIMER.md](docs/legal/DISCLAIMER.md) for the full disclaimer.

## License

MIT. See [LICENSE](LICENSE). Copyright (c) 2026 Factory Zero.

<p align="center">
  Built with <a href="https://cratefield.com">Cratefield</a> · A <a href="https://factory0.ventures">Factory Zero</a> venture
</p>
