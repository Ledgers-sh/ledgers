#!/usr/bin/env bash
# shellcheck disable=SC2064,SC2317
# Test suite for guard checks (boundary and harness-rev).
set -euo pipefail

# Resolve the repo root from this script's location so the test works from any
# working directory and in any checkout (CI, local clone, worktree).
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'
pass=0 fail=0

# expect <desc> <exit-code> <pattern>... -- <command...>
# Verify exit code and output contain all expected patterns.
expect() {
  local desc="$1" expect_code="$2"
  shift 2

  # Collect patterns until "--"
  local patterns=()
  while [ "$1" != "--" ]; do
    patterns+=("$1")
    shift
  done
  shift  # skip "--"

  local code=0
  local out
  out=$("$@" 2>&1) || code=$?

  # Check exit code
  if [ "$code" -ne "$expect_code" ]; then
    echo -e "${RED}FAIL${NC}: $desc (exit $code, expected $expect_code)"
    echo "  Output: $(echo "$out" | head -1)"
    fail=$((fail + 1))
    return
  fi

  # Check all patterns
  for pattern in "${patterns[@]}"; do
    if ! echo "$out" | grep -q "$pattern"; then
      echo -e "${RED}FAIL${NC}: $desc (pattern missing: $pattern)"
      echo "  Output: $(echo "$out" | head -1)"
      fail=$((fail + 1))
      return
    fi
  done

  echo -e "${GREEN}PASS${NC}: $desc"
  pass=$((pass + 1))
}

fixture() {
  local d
  d=$(mktemp -d)
  env -u GIT_DIR -u GIT_WORK_TREE -u GIT_INDEX_FILE git -C "$d" init -q
  env -u GIT_DIR -u GIT_WORK_TREE -u GIT_INDEX_FILE git -C "$d" config user.email "test@test.com"
  env -u GIT_DIR -u GIT_WORK_TREE -u GIT_INDEX_FILE git -C "$d" config user.name "Test"
  mkdir -p "$d/tools"
  cp "$ROOT/tools/check-boundary.sh" "$d/tools/"
  cp "$ROOT/tools/check-harness-rev.sh" "$d/tools/"
  echo "$d"
}

in_fixture() {
  local d="$1"
  shift
  (cd "$d" && env -u GIT_DIR -u GIT_WORK_TREE -u GIT_INDEX_FILE "$@")
}

git_fixture() {
  env -u GIT_DIR -u GIT_WORK_TREE -u GIT_INDEX_FILE git -C "$1" "${@:2}"
}

echo "Boundary checks:"

f=$(fixture)
trap "rm -rf '$f'" RETURN
cat > "$f/README.md" <<'EOF'
This is Ledgers-sh/ledgers-cloud, the open-core.
EOF
git_fixture "$f" add README.md
git_fixture "$f" commit -q -m "init"
expect "  clean repo" 0 "boundary ok" -- in_fixture "$f" bash tools/check-boundary.sh

f=$(fixture)
trap "rm -rf '$f'" RETURN
cat > "$f/Cargo.toml" <<'EOF'
[dependencies]
ledgers-cloud = { git = "https://github.com/x/ledgers-cloud" }
EOF
git_fixture "$f" add Cargo.toml
git_fixture "$f" commit -q -m "init"
expect "  committed Cargo.toml violation" 1 "boundary violation" "Cargo.toml:" -- \
  in_fixture "$f" bash tools/check-boundary.sh

f=$(fixture)
trap "rm -rf '$f'" RETURN
cat > "$f/README.md" <<'EOF'
README
EOF
git_fixture "$f" add README.md
git_fixture "$f" commit -q -m "init"
cat > "$f/Cargo.toml" <<'EOF'
[dependencies]
ledgers_cloud = { path = "../cloud" }
EOF
expect "  untracked Cargo.toml violation" 1 "boundary violation" -- \
  in_fixture "$f" bash tools/check-boundary.sh

f=$(fixture)
trap "rm -rf '$f'" RETURN
cat > "$f/README.md" <<'EOF'
README
EOF
git_fixture "$f" add README.md
git_fixture "$f" commit -q -m "init"
mkdir -p "$f/src"
cat > "$f/src/lib.rs" <<'EOF'
use ledgers_cloud::api;
EOF
expect "  untracked .rs violation" 1 "boundary violation" -- \
  in_fixture "$f" bash tools/check-boundary.sh

f=$(fixture)
trap "rm -rf '$f'" RETURN
mkdir -p "$f/.github/workflows"
cat > "$f/.github/workflows/ci.yml" <<'EOF'
jobs:
  build:
    uses: Ledgers-sh/ledgers-cloud/.github/workflows/deploy.yml@main
EOF
git_fixture "$f" add .github/workflows/ci.yml
git_fixture "$f" commit -q -m "init"
expect "  workflow referencing cloud" 1 "boundary violation" -- \
  in_fixture "$f" bash tools/check-boundary.sh

f=$(fixture)
trap "rm -rf '$f'" RETURN
mkdir -p "$f/src"
cat > "$f/src/index.ts" <<'EOF'
import { api } from 'ledgers-cloud-sdk';
EOF
git_fixture "$f" add src/index.ts
git_fixture "$f" commit -q -m "init"
expect "  TypeScript referencing cloud" 1 "boundary violation" -- \
  in_fixture "$f" bash tools/check-boundary.sh

echo ""
echo "Harness rev checks:"

f=$(fixture)
trap "rm -rf '$f'" RETURN
cat > "$f/Cargo.lock" <<'EOF'
version = 3
[[package]]
name = "test"
version = "0.1.0"
EOF
git_fixture "$f" add Cargo.lock
git_fixture "$f" commit -q -m "init"
mkdir -p "$f/bin"
cat > "$f/bin/git" <<'GITEOF'
#!/bin/bash
[[ "$*" == *"clone"* ]] && exit 1
exec /usr/bin/git "$@"
GITEOF
chmod +x "$f/bin/git"

CODE=0
OUT=$(cd "$f" && PATH=$PWD/bin:$PATH \
  env -u GIT_DIR -u GIT_WORK_TREE -u GIT_INDEX_FILE \
  bash tools/check-harness-rev.sh 2>&1) || CODE=$?

if [ "$CODE" -eq 0 ] && echo "$OUT" | grep -q "no git-pinned harness"; then
  echo -e "${GREEN}PASS${NC}: crates.io-only (skip, no clone)"
  pass=$((pass + 1))
else
  echo -e "${RED}FAIL${NC}: crates.io-only (skip, no clone)"
  echo "  Output: $(echo "$OUT" | head -1)"
  fail=$((fail + 1))
fi

f=$(fixture)
trap "rm -rf '$f'" RETURN
cat > "$f/Cargo.lock" <<'EOF'
version = 3
[[package]]
name = "cratefield-core"
version = "0.1.0"
source = "git+https://github.com/Cratefield/harness?branch=main"
EOF
git_fixture "$f" add Cargo.lock
git_fixture "$f" commit -q -m "init"
expect "  floating branch pin (fail)" 1 "floating git pin" -- \
  in_fixture "$f" bash tools/check-harness-rev.sh

echo ""
echo "Test Results: ${GREEN}$pass passed${NC}, ${RED}$fail failed${NC}"
exit "$fail"
