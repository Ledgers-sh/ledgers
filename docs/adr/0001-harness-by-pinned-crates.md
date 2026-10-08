# 0001: Depend on the Cratefield harness by pinned crates, not a submodule

Date: 2026-10-09. Status: accepted.

## Context

Ledgers is built on the Cratefield harness. The options were a git submodule
of `Cratefield/harness`, a git dependency by `rev`, published crates.io
versions, or living inside the harness repository under `ventures/`.

## Decision

Published crates.io versions, pinned exactly (`=x.y.z`) in
`[workspace.dependencies]` and bumped together in one PR. Crates the harness
has not published yet come from git at one pinned `rev`, with
`[patch.crates-io]` pointing `cratefield-core` at the same `rev` so there is
one copy of core. CI verifies the `rev` is an ancestor of harness `main` and
that `cargo tree -d` lists one `cratefield-core`. Each git dependency is moved
to crates.io as soon as it is published.

## Consequences

- Ledgers crates can be published and `cargo install`ed (path dependencies into
  a submodule cannot be published).
- Same pattern as every other Factory Zero venture (Owlpost, AlphaHunt, Living
  Brain); no venture uses a submodule.
- Harness changes Ledgers needs go upstream as harness PRs first; local work
  uses an uncommitted `[patch.crates-io]` to a local harness clone.
- A harness bump is a visible, reviewed change.
