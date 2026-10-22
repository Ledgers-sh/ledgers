#!/usr/bin/env bash
# Quick-start smoke test: starts ledgers-server with LEDGERS_DEV=1 on a free
# port, takes the dev key it prints, and walks the README curl sequence:
# post an entry, trial balance, P&L, reverse, trial balance again.
# Exits non-zero on the first failed check. Writes only under a temp dir.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="${CARGO_TARGET_DIR:-$ROOT/target}"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/ledgers-smoke.XXXXXX")"
SERVER_PID=""

cleanup() {
  if [ -n "$SERVER_PID" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  rm -rf "$WORK"
}
trap cleanup EXIT

fail() {
  echo "smoke: FAIL: $*" >&2
  echo "--- server stderr ---" >&2
  tail -n 20 "$WORK/server.err" >&2 || true
  exit 1
}

check_json() {
  # check_json <description> <python expression over the variable `d`> <json>
  local description="$1" expression="$2" body="$3"
  if ! printf '%s' "$body" | python3 -I -c "
import json, sys
d = json.load(sys.stdin)
sys.exit(0 if ($expression) else 1)
"; then
    fail "$description (body: $body)"
  fi
  echo "smoke: ok: $description"
}

free_port() {
  python3 -I -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()'
}

echo "smoke: building ledgers-server"
(cd "$ROOT" && cargo build -q -p ledgers-server)
BIN="$TARGET/debug/ledgers-server"
[ -x "$BIN" ] || fail "binary not built at $BIN"

PORT="$(free_port)"
BASE="http://127.0.0.1:$PORT"
echo "smoke: starting the server on 127.0.0.1:$PORT (LEDGERS_DEV=1)"
env -u HARNESS_SECRET \
  LEDGERS_DEV=1 \
  LEDGERS_DEMO=0 \
  LISTEN_ADDR="127.0.0.1:$PORT" \
  DATABASE_PATH="$WORK/ledgers.db" \
  BLOB_DIR="$WORK/blobs" \
  "$BIN" >"$WORK/server.out" 2>"$WORK/server.err" &
SERVER_PID=$!

# The dev key is printed once, on start.
KEY=""
for _ in $(seq 1 120); do
  KEY="$(sed -n 's/^dev key: //p' "$WORK/server.out" | head -n 1)"
  [ -n "$KEY" ] && break
  kill -0 "$SERVER_PID" 2>/dev/null || fail "the server exited before printing the dev key"
  sleep 0.5
done
[ -n "$KEY" ] || fail "no 'dev key:' line within 60 seconds"
grep -q "HARNESS_SECRET is unset" "$WORK/server.err" \
  || fail "the throwaway HARNESS_SECRET warning was not logged"
echo "smoke: ok: dev key printed once; throwaway HARNESS_SECRET warned"

# Wait until the HTTP listener answers (401 without a credential).
for _ in $(seq 1 120); do
  code="$(curl -s -o /dev/null -w '%{http_code}' "$BASE/v1/accounts" || true)"
  [ "$code" = "401" ] && break
  sleep 0.5
done
[ "$code" = "401" ] || fail "the listener did not answer 401 without a credential (got $code)"
echo "smoke: ok: no credential is refused with 401"

call() {
  # call <method> <path> [json-body]; sets STATUS and BODY
  local method="$1" path="$2" data="${3:-}"
  local out
  if [ -n "$data" ]; then
    out="$(curl -sS -X "$method" "$BASE$path" -H "Authorization: Bearer $KEY" \
      -H 'Content-Type: application/json' --data "$data" -w '\n%{http_code}')"
  else
    out="$(curl -sS -X "$method" "$BASE$path" -H "Authorization: Bearer $KEY" -w '\n%{http_code}')"
  fi
  STATUS="${out##*$'\n'}"
  BODY="${out%$'\n'*}"
}

TODAY="$(date -u +%Y-%m-%d)"
PERIOD="${TODAY:0:7}"
EXT="quickstart-$(date -u +%s)"

ENTRY=$(cat <<JSON
{"entry": {
  "journal_id": "",
  "document_date": "$TODAY",
  "posting_date": "$TODAY",
  "tax_point_date": null,
  "base_currency": "EUR",
  "description": "Quick start: consulting invoice",
  "source_documents": [],
  "external_id": "$EXT",
  "source_app": "quickstart",
  "reverses": null,
  "lines": [
    {"account_number": "5010", "side": "debit", "base": {"minor": "100000", "currency": "EUR"},
     "fx": null, "tax_code": null, "dimensions": {}, "memo": null},
    {"account_number": "4000", "side": "credit", "base": {"minor": "100000", "currency": "EUR"},
     "fx": null, "tax_code": null, "dimensions": {}, "memo": null}
  ]
}}
JSON
)

call POST /v1/entries "$ENTRY"
[ "$STATUS" = "200" ] || fail "post entry: HTTP $STATUS"
check_json "post entry 1000.00 EUR (5010 debit, 4000 credit)" "d['entry']['number'] >= 1" "$BODY"

call GET "/v1/reports/trial-balance?period=$PERIOD"
[ "$STATUS" = "200" ] || fail "trial balance: HTTP $STATUS"
check_json "trial balance balances after posting" \
  "sum(int(r['debit']['minor']) for r in d['rows']) == sum(int(r['credit']['minor']) for r in d['rows']) == 100000" "$BODY"

call GET "/v1/reports/p-and-l?period=$PERIOD"
[ "$STATUS" = "200" ] || fail "profit and loss: HTTP $STATUS"
check_json "P&L shows 1000.00 EUR revenue" "d['revenue']['minor'] == '100000'" "$BODY"

call POST "/v1/entries/$EXT/reverse" "{}"
[ "$STATUS" = "200" ] || fail "reverse entry: HTTP $STATUS"
check_json "reversal posted" "d['entry']['number'] >= 2" "$BODY"

call GET "/v1/reports/trial-balance?period=$PERIOD"
[ "$STATUS" = "200" ] || fail "trial balance after reversal: HTTP $STATUS"
check_json "trial balance nets to zero after the reversal" \
  "all(r['balance']['minor'] == '0' for r in d['rows'])" "$BODY"

echo "smoke: all checks passed"
