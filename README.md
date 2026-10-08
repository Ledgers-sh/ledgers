# Ledgers

Open-source accounting built for AI agents. Agents read invoices, match
payments and draft journal entries; a policy decides what they may post; a
person approves the rest, with a passkey for anything high-risk. Every action
is audited and reversible. The app, the CLI, MCP tools and the HTTP API run
the same commands.

**Status: planned. Nothing is built or published yet.** Do not
`cargo install ledgers`: that name on crates.io belongs to an unrelated
project. Our crates will be named `ledgers-*`.

- The plan: [docs/PLAN.md](docs/PLAN.md)
- Decisions: [docs/adr/](docs/adr/)
- The work: this repository's issues, grouped by milestone (M0 to M5)
- Site: https://ledgers.sh

Built on the [Cratefield](https://cratefield.com) harness. MIT licensed.
Made by [Factory Zero](https://factory0.ventures).
