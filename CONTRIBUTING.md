# Contributing to Ledgers

## Bounties and outside pull requests

There are no bounties on this project. Issues here are the team's planning notes, not paid tasks.
Pull requests from people outside the team are only reviewed when the change was discussed in the
issue first and a maintainer agreed to it. Other pull requests are closed without review.

The short path to a green PR. The rules behind it live in
[docs/PLAN.md](docs/PLAN.md) (section 3 is not negotiable) and the
[ADRs](docs/adr/).

## Toolchain

The toolchain is pinned in [rust-toolchain.toml](rust-toolchain.toml); rustup
installs Rust 1.98.1 automatically the first time you run `cargo`. You also
need:

- the `wasm32-unknown-unknown` target — rust-toolchain.toml lists it, so
  rustup installs it with the toolchain (`rustup target add
  wasm32-unknown-unknown` if your machine skipped it),
- [cargo-deny](https://github.com/EmbarkStudios/cargo-deny) for the licence
  and ban checks: `cargo install cargo-deny`, or grab a
  [prebuilt release](https://github.com/EmbarkStudios/cargo-deny/releases).

## Checks

Every PR must pass all of these — CI runs exactly this list:

```
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check --target wasm32-unknown-unknown -p ledgers-server
cargo deny check
bash tools/test-guards.sh
bash tools/check-boundary.sh
bash tools/check-harness-pins.sh
bash tools/check-harness-rev.sh
```

The last three enforce the structural rules:

- `tools/check-boundary.sh` — no code, manifest or workflow may reference the
  cloud repository ([ADR 0002](docs/adr/0002-open-core-boundary.md)).
- `tools/check-harness-pins.sh` — the graph must contain exactly one
  `cratefield-core` ([ADR 0001](docs/adr/0001-harness-by-pinned-crates.md)).
- `tools/check-harness-rev.sh` — every git-pinned harness rev must be an
  ancestor of `Cratefield/harness` `main`
  ([ADR 0001](docs/adr/0001-harness-by-pinned-crates.md)).

## Harness pins

Every `cratefield-*` crate is pinned exactly (`=`) in
`[workspace.dependencies]`, and the whole set is bumped together in one PR
([ADR 0001](docs/adr/0001-harness-by-pinned-crates.md)) — never one crate on
its own. `cargo tree -d` must show a single `cratefield-core`; CI enforces it
with `tools/check-harness-pins.sh`.

## Working against a local harness clone

Harness changes Ledgers needs go upstream as harness PRs first. While you
wait, point this workspace at your clone with a `[patch.crates-io]` section
in Cargo.toml, covering the `cratefield-*` crates your change touches:

```toml
[patch.crates-io]
cratefield-core = { path = "../harness/crates/core" }
```

Keep that section **uncommitted**: it must never land in a PR or on main.
In practice: do not `git add Cargo.toml` while it exists, `git stash` it
before committing, or run `git update-index --skip-worktree Cargo.toml` for
the duration (and `--no-skip-worktree` to undo it once the pin works again).

## Unpublished harness crates

`cratefield-auth-passkeys` and `cratefield-mcp` are not on crates.io yet.
They will arrive as git dependencies from one pinned rev of
`Cratefield/harness`, with `[patch.crates-io]` pointing `cratefield-core` at
the same rev so the build keeps exactly one copy of core. CI checks the
pinned rev is an ancestor of harness main (`tools/check-harness-rev.sh`).
Each git dependency moves to a crates.io `=` pin as soon as the harness
publishes it, in the same one-PR release-round bump.

## Open-core boundary

This repository is the complete MIT core for one company. What is about more
people, more companies or running costs lives in a separate private
repository ([ADR 0002](docs/adr/0002-open-core-boundary.md)). Code, manifests
and workflows here must never reference it — `tools/check-boundary.sh` fails
the build if they do. If prose genuinely needs to mention it, use a `.md`
file.

## Money crates

When `ledgers-money` and `ledgers-ledger` are created, their crate roots
carry `#![deny(clippy::float_arithmetic)]`. Workspace lints are shared and
cannot vary per crate, so the stricter rule lives in the crates it protects:
money is computed in integers, never floats.
