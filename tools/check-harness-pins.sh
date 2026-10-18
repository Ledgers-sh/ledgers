#!/usr/bin/env bash
# Harness pin check (docs/adr/0001-harness-by-pinned-crates.md).
#
# The workspace must resolve to exactly one cratefield-core version: every
# cratefield-* crate is pinned with '=' in [workspace.dependencies] and the
# whole round is bumped together in one PR. A second (or missing) copy of
# core in the dependency graph means a pin was missed or a stray requirement
# slipped in.
set -euo pipefail
cd "$(dirname "$0")/.."

if ! tree_output=$(cargo tree --workspace --target=all 2>&1); then
  echo "harness pin check: 'cargo tree --workspace --target=all' failed, cannot verify the cratefield-core pin:" >&2
  printf '%s\n' "$tree_output" | sed 's/^/  /' >&2
  exit 1
fi

# The extracted token is the full semver INCLUDING the parenthesized source,
# so two copies of cratefield-core with the same version but different
# sources (two git revs both claiming v0.1.0, or a stray path copy next to
# the registry pin) are still told apart — that is the ADR 0001
# unpublished-crates scenario. cargo tree marks repeats of an already-printed
# package with (*); those carry no source and are dropped.
versions=$(
  printf '%s\n' "$tree_output" \
    | grep -oE 'cratefield-core v[^ ]+( \([^)]*\))?' \
    | grep -vF ' (*)' \
    | sort -u || true
)
count=$(printf '%s\n' "$versions" | grep -c . || true)

if [ "$count" -ne 1 ]; then
  echo "harness pin violation: expected exactly one cratefield-core version in the dependency graph, found $count" >&2
  if [ -n "$versions" ]; then
    printf '%s\n' "$versions" | sed 's/^/  /' >&2
  fi
  echo "All cratefield-* crates must come from one release round pinned with '=' in [workspace.dependencies] and be bumped together in one PR (docs/adr/0001-harness-by-pinned-crates.md)." >&2
  exit 1
fi

echo "harness pins ok: exactly one cratefield-core ($versions)"
