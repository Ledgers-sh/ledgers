#!/usr/bin/env bash
# Links the Worker's wasm32 artifact (ledgers-server is a cdylib), so a link
# failure fails here, before a deploy. `cargo check` never links, so it cannot
# catch these: the `ring` crate's C parts need a clang that can target wasm32.
#
# The C compiler and archiver come from CC_wasm32_unknown_unknown and
# AR_wasm32_unknown_unknown. If they are not set, this picks a wasm32-capable
# clang (Homebrew LLVM on macOS, or clang on the PATH) and its llvm-ar.
# Apple's clang has no wasm32 target; see docs/deploy.md, "Toolchain for the Worker build".
#
# Usage: tools/check-wasm-link.sh
set -euo pipefail
cd "$(dirname "$0")/.."

# A clang that can link wasm32 for ring. Apple's clang compiles wasm32 objects,
# but its output fails to link against ring's C parts, so it is never picked:
# Homebrew LLVM is tried first, then a non-Apple clang on the PATH.
usable_clang() {
  local clang="$1"
  [ -x "$clang" ] || command -v "$clang" >/dev/null 2>&1 || return 1
  "$clang" --version 2>/dev/null | head -1 | grep -q '^Apple clang' && return 1
  "$clang" --print-targets 2>/dev/null | grep -q '^ *wasm32 ' || return 1
  local probe
  probe="$(mktemp -d)"
  printf 'int ledgers_wasm_probe(void) { return 0; }\n' > "$probe/probe.c"
  "$clang" --target=wasm32-unknown-unknown -c "$probe/probe.c" -o "$probe/probe.o" >/dev/null 2>&1
  local status=$?
  rm -rf "$probe"
  return $status
}

if [ -z "${CC_wasm32_unknown_unknown:-}" ]; then
  candidates=()
  if command -v brew >/dev/null 2>&1; then
    candidates+=("$(brew --prefix llvm 2>/dev/null || true)/bin/clang")
  fi
  candidates+=("/opt/homebrew/opt/llvm/bin/clang" "/usr/local/opt/llvm/bin/clang" "clang")
  for candidate in "${candidates[@]}"; do
    if usable_clang "$candidate"; then
      export CC_wasm32_unknown_unknown="$candidate"
      break
    fi
  done
fi

if [ -z "${CC_wasm32_unknown_unknown:-}" ]; then
  echo "check-wasm-link: no usable wasm32 clang found (Apple clang is not used for ring)." >&2
  echo "  macOS: brew install llvm." >&2
  echo "  Linux: apt-get install clang llvm." >&2
  exit 1
fi

if [ -z "${AR_wasm32_unknown_unknown:-}" ]; then
  clang_dir="$(dirname "$CC_wasm32_unknown_unknown")"
  if [ -x "$clang_dir/llvm-ar" ]; then
    export AR_wasm32_unknown_unknown="$clang_dir/llvm-ar"
  elif command -v llvm-ar >/dev/null 2>&1; then
    export AR_wasm32_unknown_unknown="$(command -v llvm-ar)"
  else
    echo "check-wasm-link: llvm-ar not found next to $CC_wasm32_unknown_unknown or on the PATH." >&2
    exit 1
  fi
fi

echo "check-wasm-link: CC=$CC_wasm32_unknown_unknown AR=$AR_wasm32_unknown_unknown"
cargo build --release --locked --target wasm32-unknown-unknown -p ledgers-server

artifact="target/wasm32-unknown-unknown/release/ledgers_server.wasm"
if [ ! -s "$artifact" ]; then
  echo "check-wasm-link: $artifact was not produced." >&2
  exit 1
fi
echo "check-wasm-link: linked $artifact ($(wc -c < "$artifact" | tr -d ' ') bytes)"
