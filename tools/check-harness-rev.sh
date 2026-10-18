#!/usr/bin/env bash
# Harness git-rev check (docs/adr/0001-harness-by-pinned-crates.md).
#
# Harness crates that are not on crates.io yet come from
# Cratefield/harness at one pinned rev. A rev that is not an ancestor of
# harness main is a throwaway or force-pushed-away commit: rebase the
# harness branch onto main and pin the new rev.
#
# Skips cleanly (exit 0) when the lockfile pins no git harness dependency —
# that is the normal state while every harness crate is on crates.io.
set -euo pipefail
cd "$(dirname "$0")/.."

lock="${CARGO_LOCK:-Cargo.lock}"
repo="${HARNESS_REPO:-https://github.com/Cratefield/harness.git}"

if [ ! -f "$lock" ]; then
  echo "harness rev check: $lock not found; no git-pinned harness dependencies to verify"
  exit 0
fi

revs=$(
  grep -oE 'git\+https://github\.com/Cratefield/harness(\.git)?\?rev=[0-9a-f]{40,64}' "$lock" \
    | sed -E 's/.*rev=//' | sort -u || true
)

# A harness git source pinned with branch= or tag= instead of rev= would slip
# past the rev extraction above and silently skip the ancestor check. A
# floating pin is exactly what ADR 0001 forbids: one pinned rev, so any
# harness git source line without rev= is a violation. This runs BEFORE the
# no-revs early exit so a branch-only pin cannot skip the check.
floating=$(
  grep -iE 'git\+[^"]*cratefield/harness' "$lock" | grep -v 'rev=' || true
)

if [ -n "$floating" ]; then
  echo "harness rev violation: floating git pin for Cratefield/harness (branch= or tag= instead of one pinned rev):" >&2
  printf '%s\n' "$floating" | sed 's/^/  /' >&2
  echo "ADR 0001 requires one pinned rev that is an ancestor of harness main; pin ?rev=<sha> instead (docs/adr/0001-harness-by-pinned-crates.md)." >&2
  exit 1
fi

if [ -z "$revs" ]; then
  echo "harness rev check: no git-pinned harness dependencies in $lock; nothing to verify"
  exit 0
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
git clone --quiet --filter=blob:none --no-checkout "$repo" "$tmp/harness"
git -C "$tmp/harness" fetch --quiet origin main

fail=0
for rev in $revs; do
  git -C "$tmp/harness" fetch --quiet origin "$rev"
  if git -C "$tmp/harness" merge-base --is-ancestor "$rev" origin/main; then
    echo "harness rev ok: $rev is an ancestor of Cratefield/harness main"
  else
    echo "harness rev violation: $rev is NOT an ancestor of Cratefield/harness main" >&2
    echo "Rebase the harness branch onto main and pin the new rev (docs/adr/0001-harness-by-pinned-crates.md)." >&2
    fail=1
  fi
done

exit $fail
