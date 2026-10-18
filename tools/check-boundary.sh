#!/usr/bin/env bash
# Open-core boundary check (docs/adr/0002-open-core-boundary.md).
#
# The MIT core must never reference the private cloud repository: not from
# code, not from manifests, not from CI. A reference in prose is fine — that
# is why only *.toml, *.rs, *.ts and .github/workflows/ are scanned, and .md
# files are exempt.
set -euo pipefail
cd "$(dirname "$0")/.."

if ! git rev-parse --quiet --is-inside-work-tree >/dev/null 2>&1; then
  echo "boundary check: not a git repository, cannot scan" >&2
  exit 1
fi

pattern='ledgers[-_]cloud'
paths=('*.toml' '*.rs' '*.ts' '.github/workflows')

# Tracked files via git grep, plus untracked-but-present files so an
# uncommitted violation is caught before it ever reaches a commit.
violations=$(
  {
    git grep -iI -n -E "$pattern" -- "${paths[@]}" 2>/dev/null || true
    untracked=$(git ls-files --others --exclude-standard -- "${paths[@]}")
    if [ -n "$untracked" ]; then
      printf '%s\n' "$untracked" | tr '\n' '\0' \
        | xargs -0 grep -iInE "$pattern" -- 2>/dev/null || true
    fi
  } | sort -u
)

if [ -n "$violations" ]; then
  echo "boundary violation: code, manifests and workflows must not reference the cloud repo" >&2
  echo "(docs/adr/0002-open-core-boundary.md). Offending file:line:" >&2
  printf '%s\n' "$violations" | sed 's/^/  /' >&2
  echo "If this reference is legitimate documentation, put it in a .md file; code, manifests and workflows must stay clean." >&2
  exit 1
fi

echo "boundary ok: no cloud-repo references in code, manifests or workflows (tracked or uncommitted)"
