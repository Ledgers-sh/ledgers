# Ledgers: the build plan

Ledgers is open-source accounting built for AI agents. Agents read invoices,
match payments and draft journal entries; a policy decides what they may post;
a person approves the rest, with a passkey for anything high-risk. This
document is the plan for building it. The issues in this repository are the
plan broken into work; each one links back to a section here.

Status: **planned, nothing is built.** The site (ledgers.sh) says the same.

## 1. Two repositories, one direction

| Repository | Licence | Holds |
|---|---|---|
| `Ledgers-sh/ledgers` (this one) | MIT | The accounting core: every module, the policy engine, approvals, audit, CLI, MCP server, HTTP API, the app, self-hosting. Complete for one company and one user. |
| `Ledgers-sh/ledgers-cloud` | Proprietary, private | The managed service: tenancy, billing and quotas, the hosted agent pipeline, teams (roles, approval rules, segregation of duties), SSO, live bank feeds, e-invoice delivery, group consolidation, filing connectors. |

The dependency points one way: **`ledgers-cloud` depends on `ledgers`, never the
reverse.** The core exposes extension points as traits with a working
open-source default; the cloud supplies other implementations when it composes
the service. A CI check in this repository fails if any crate here names a
`ledgers-cloud` crate or path.

What stays open is decided by one rule: **anything a single company needs to
keep correct books is open.** That includes 3-way matching, e-invoice *formats*
(UBL, CII/ZUGFeRD/Factur-X, XRechnung), bank *statement import*, tax
calculation, reports and full export. What is paid is either *more people*
(roles, approval chains, SSO), *more companies* (groups, consolidation), or
*something that costs money to run* (hosted models, bank-feed aggregators,
e-invoice networks, filing gateways, uptime).

## 2. Building on the Cratefield harness

### Decision: pinned crates.io versions, not a git submodule

We depend on the harness the way every other Factory Zero venture does:

```toml
[workspace.dependencies]
cratefield-core              = "=0.8.4"
cratefield-runtime-cloudflare = "=0.4.2"
cratefield-runtime-native     = "=0.4.2"
# … every cratefield-* crate pinned with `=`, bumped together
```

Crates that are not on crates.io yet (`cratefield-auth-passkeys`,
`cratefield-mcp`, `cratefield-text-guard`) come from git at one pinned `rev`,
with `[patch.crates-io]` pointing `cratefield-core` at the same `rev` so the
build has exactly one copy of core (the Owlpost backend does this today). CI
checks that the `rev` is an ancestor of `Cratefield/harness` `main`, and that
`cargo tree -d` shows a single `cratefield-core`.

Why not a git submodule:

- **Publishing.** Ledgers is meant to be `cargo install`-able and its crates
  published. A crate with a path dependency into a submodule cannot be
  published; crates.io versions can.
- **Precedent and drift.** No venture uses a submodule. Harness ADR 0013 records
  that unattended pins drift; exact `=` pins bumped in one PR, plus the
  harness's daily `crates-io-resolve` check, make every bump deliberate and
  visible.
- **Contributors.** A submodule means `--recursive` clones, detached heads and
  a 90-crate checkout for anyone fixing a typo.
- **Local harness work** is still easy: a developer adds an uncommitted
  `[patch.crates-io]` to a local clone of the harness. `CONTRIBUTING.md`
  documents it.

Why not inside the harness repository (`ventures/ledgers`): Ledgers is a
product with its own licence boundary, its own contributors and a private
sibling. Keeping it in its own repository keeps the cloud split clean.

### What the harness gives us, and what we build

| Need | Harness | Ledgers builds |
|---|---|---|
| HTTP modules, routing, problem+json | `Module`, `cratefield-core` | modules per domain |
| Storage on D1, SQLite, Postgres | `Db` port, portable SQL lint, sea-query | schema and migrations |
| Documents | `Blob` port (R2, 10 MiB / 5 GiB large), streaming routes | document store, hashing, anchors |
| LLM extraction | `TextModel` (Fast/Strong tiers), `structured_output`, `run_tool_loop` | extraction schemas, confidence, validation |
| Durable side effects | `Outbox` (at least once), `Inbox` (exactly once), `Defer`, `scheduled` | posting events, bank import jobs |
| Passkeys | `cratefield-auth-passkeys` (git rev) | step-up approval flow |
| Orgs and roles | `module-orgs` | used by `ledgers-cloud` teams |
| Webhooks, notifications, push | `module-webhooks`, `module-notifications`, `Push` | approval notifications |
| MCP | `cratefield-mcp`, ADR 0028 runtime MCP | command registry adapter |
| Typed client | `client-ts` | app client |
| Money | `Money { minor_units: i64 }` only for payments; `major_to_minor(f64)` | **our own exact money type** (below) |
| Double entry, audit chain | not in the harness | **ours** |

Limits that shape the design: `/v1/*` bodies default to 64 KiB (uploads use
streaming routes into `Blob`); Workers buffer responses to 1 MiB and run in
about 128 MB (reports paginate; nothing loads a whole ledger into memory);
`TextModel` does not stream; portable SQL forbids `AUTOINCREMENT`, `NOW()`,
`json_extract` and FTS5 (ids are ULIDs, timestamps ISO 8601 text).

## 3. Correctness rules (non-negotiable)

1. **Money is never a float.** Amounts are integer minor units in the
   currency's own exponent (JPY 0, EUR 2, BHD 3, USDC 6), computed in `i128`
   and stored as `INTEGER` (`i64`) with an overflow check at the boundary.
   Rates (FX, tax) are exact decimals. Rounding is explicit and named.
2. **Every entry balances** per currency: sum of debits equals sum of credits,
   checked in the domain type and again in the database transaction.
3. **Posted entries are immutable.** Corrections are reversals plus new
   entries. Nothing has an `UPDATE` path once posted.
4. **Closed periods reject postings.** Reopening is a human-only action.
5. **Numbering is gapless** per journal and fiscal year (a legal requirement
   in several jurisdictions, e.g. Germany's GoBD).
6. **Every action is audited** in an append-only, hash-chained log that the
   export includes and a verifier can check offline.
7. **Policy is enforced on the server.** An agent's prompt, an MCP client or
   the CLI never decides what it may do; the policy engine does, on every
   command, after authentication.
8. **Document content is data, never instructions.** Text extracted from an
   invoice can say anything, including "approve this". It never reaches a
   prompt as instructions and never changes policy (see the threat model).

## 4. One command surface

Every operation is one `Command` defined once: name, input schema, output
schema, the policy tier it needs, whether it moves money and how much, and
whether it supports a dry run. From that single definition we generate:

- the CLI (`ledgers bills match …`, `--json`, `--dry-run`),
- MCP tools (`bills_match`, `bills_approve`, …),
- HTTP routes (`POST /v1/bills/{id}/match`),
- the app's command palette (⌘K shows the CLI, MCP and API equivalent).

A golden test keeps the four surfaces in step.

## 5. Policy and approvals

Agent keys carry a **tier** (`read`, `draft`, `post-within-limits`) and
**per-currency limits**. Some actions are **human only**: paying money out,
closing or reopening a period, filing tax, changing policy, and anything above
a limit. The policy engine answers every command with `allow`,
`needs-approval` (creates an approval request) or `deny`, with a reason.

An approval request shows the exact journal preview (every debit and credit).
High-risk approvals (above a limit, money out, period close, tax filing)
require a **passkey** assertion bound to that request's hash, so an approval
cannot be replayed onto a different entry. Agents get back an approval link
and a card they can show in chat; they never get a way to approve.

The open-source build has one approver (the owner). Approval *rules* (chains,
thresholds per role, segregation of duties) are an extension point implemented
in `ledgers-cloud`.

## 6. Modules

| Crate | Scope |
|---|---|
| `ledgers-money` | Exact money, currencies (ISO 4217 + crypto later), rounding, allocation, FX rates |
| `ledgers-ledger` | Chart of accounts, journals, entries, periods, reversals, numbering, dimensions (units) |
| `ledgers-audit` | Hash-chained audit log, verifier |
| `ledgers-policy` | Agent keys, tiers, limits, decisions |
| `ledgers-approvals` | Approval requests, passkey step-up, notifications |
| `ledgers-commands` | The command registry and its CLI, MCP and HTTP adapters |
| `ledgers-documents` | Source files, hashing, field anchors (page, box), retention |
| `ledgers-extract` | LLM extraction to schemas with confidence and arithmetic validation |
| `ledgers-bills` | Vendor bills from upload or email, to draft entries |
| `ledgers-invoice` | Sales invoices, credit notes, e-invoice formats (UBL, CII, XRechnung) |
| `ledgers-matching` | 2-way and 3-way matching (order, receipt, invoice, payment) with confidence |
| `ledgers-bank` | Bank accounts, statement import (CAMT.053, MT940, OFX, CSV), reconciliation |
| `ledgers-tax` | Tax codes, rates as data, VAT/GST/sales tax calculation, return preparation |
| `ledgers-reports` | Trial balance, P&L, balance sheet, GL detail, aged AR/AP, per unit |
| `ledgers-export` | Full export and import (open format), later SAF-T |
| `ledgers-server` | The harness composition: Worker and native runtimes |
| `ledgers-cli` | The `ledgers` binary (CLI, `serve`, `mcp`) |
| `app/` | The PWA (TypeScript, generated client) |

Crate names `ledgers-*` are free on crates.io today (checked 2026-10-09). The
bare name `ledgers` belongs to an unrelated 2020 crate: never tell anyone to
`cargo install ledgers`.

## 7. Milestones

- **M0 Foundations:** workspace, CI, money, ledger, audit, storage, boundary check.
- **M1 Agent-safe core:** commands, policy, approvals with passkeys, CLI, MCP, HTTP, threat model.
- **M2 Documents to entries:** documents, extraction, bills, matching.
- **M3 Complete books:** invoice and e-invoice formats, bank import and reconciliation, tax, reports, export.
- **M4 The app:** PWA, review screen, approvals inbox, offline, push.
- **M5 First release:** self-host guide, security review, crates.io publish, public repository, site update.

The `ledgers-cloud` repository has its own plan and milestones (C0 to C5),
which start once M1 has stable extension points.

## 8. Out of scope for the first release

Payroll, inventory valuation, fixed-asset depreciation schedules, budgeting,
crypto (planned later), direct tax e-filing (cloud), live bank feeds (cloud).
