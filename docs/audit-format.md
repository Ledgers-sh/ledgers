# The audit format: canonical JSON, the chain, and the export

The audit chain (docs/PLAN.md §3 rule 6) is a hash-chained log. This
document is the specification a third party could implement a verifier
from; `crates/ledgers-audit` is the implementation, and its test suite
pins every vector below so the two cannot drift.

## Canonical JSON

One byte sequence per value, so a hash over "the event" is a hash over
*the* event. This is **RFC 8785** (JSON Canonicalization Scheme)
restricted to the **integer-only subset** — the same escaping set and
the same key-ordering rule, with no float handling.

- UTF-8. No byte-order mark, no insignificant whitespace.
- Object keys sort by the byte order of their UTF-8 encoding, at every
  depth. Rust's `str` ordering is already byte-wise.
- Strings escape only `"`, `\` and U+0000–U+001F: `\b \t \n \f \r`
  where a two-char form exists, `\u00xx` (lowercase hex) for the rest.
  `/` is **not** escaped; non-ASCII is written **literally** — a
  `Müller` is its bytes, never `ü`.
- Numbers are integers in plain decimal. A float is an **error**, not a
  rounding decision: no number a ledger audits is a float
  (docs/PLAN.md §3 rule 1), and hashing one would bind the chain to its
  shortest round-trip form.
- `true`, `false`, `null` literal.

Canonical JSON is written by this crate's own serialiser, not
`serde_json`'s: `serde_json::Map` is a `BTreeMap` or an `IndexMap`
depending on the `preserve_order` feature, which feature unification in
a workspace can enable under a crate that never asked, and the same
event would then hash two ways.

## The event

Every field is present in the canonical bytes; an absent optional is
`null`, never missing. Deserialization is strict (`deny_unknown_fields`):
a verifier that skipped fields it did not understand would verify a
document it had not read.

| Field | Type | Notes |
|---|---|---|
| `id` | string | ULID, injected by the caller |
| `company_id` | string | constant across a chain |
| `seq` | integer | 1-based, gapless |
| `at` | string | RFC 3339 UTC, injected |
| `actor` | object | see below |
| `command` | string | dotted name, e.g. `journal.post_entry` |
| `input_hash` | string | lowercase hex SHA-256 of the canonical input |
| `outcome` | string | `allowed` / `needs-approval` / `denied` / `error` |
| `objects` | array of strings | ids of the objects touched |
| `reason` | string or null | the policy engine's reason |
| `approval_id` | string or null | the approval request bound |
| `request_id` | string | joins the chain to the access log |

**Actor.** `{"kind":"person","person_id":"…"}` or
`{"kind":"agent","key_id":"…","tier":"…"}` where tier is `read`,
`draft` or `post-within-limits`. Note there is **no field named `id`
inside the actor**: the verifier's fallback for an unparseable line
scans for the first `"id":"` in the raw bytes, and that must be the
event's own id.

## The hash and genesis

    event_hash(prev_hash, event) = SHA-256( prev_hash_ascii_bytes
                                           || canonical_json(event) )

rendered lowercase hex. `prev_hash` is inside the digest, so an event
cannot be moved in the chain without changing its own hash. The
concatenation is unambiguous: `prev_hash` is a 64-hex digest (or the
one-character genesis `"0"`) and canonical JSON of an event always
begins with `{`.

The genesis `prev_hash` is `ledgers_db::chain::GENESIS_HASH`, `"0"` —
the conventional chain anchor, and checkable from the export alone.

`input_hash` is SHA-256 over the canonical JSON of the command input;
the input itself is never stored.

## Chain, seq and the fork guard

`audit_event` has `PRIMARY KEY (company_id, seq)`. An append claims
`head.seq + 1` and links `head.hash`, so two writers that read the same
head both take the same `seq`: one commits, the other takes a **unique
violation** that aborts its whole batch, which is the one failure class
`ledgers_db::retry` retries (docs/adr/0004-guard-statements.md). A fork
is impossible by storage, and the chain has no gaps because a rolled
back batch takes its `seq` with it.

## Export format

JSON Lines. One line per event in `seq` order, each **exactly** the
canonical JSON of

    {"event": <event>, "hash": "<hex>", "prev_hash": "<hex>"}

and each terminated by `\n`, including the last. The export is the
complete chain: a partial export is a file the verifier must reject,
since its first line's `seq` would not be 1.

## Verification

`verify` stops at the **first** break and reports the line, the `seq`
and the event id it could recover. Per line: valid UTF-8; valid JSON;
**the bytes equal their own canonical re-serialisation**; `seq` is 1
then previous + 1; `company_id` constant; `prev_hash` is genesis then
the previous `hash`; `hash == event_hash(prev_hash, event)`; the event
deserialises strictly. An empty export is valid — zero events, head seq
0. A missing final newline **is** a break: every line is terminated, so
its absence means truncation.

Break kinds: `NotUtf8`, `NotJson`, `NotCanonical`, `NotALine`,
`SeqGap`, `ForeignCompany`, `BadGenesis`, `BrokenLink`,
`HashMismatch`, `MissingFinalNewline`.

Because the hash covers `prev_hash` and the whole event, a rewritten
event needs a rewritten hash and every hash after it — and with a
published anchor, even a rewritten tail is detectable.

## Anchors

An **anchor** publishes the chain head at a known `seq`, so a deleted
tail cannot go unnoticed. Topic `audit.anchor`, payload the canonical
JSON `{"company_id":…,"hash":…,"seq":…}`, outbox id
`{event_id}-anchor` (deterministic, so a retried batch never enqueues
twice). Written every `anchor_every` events (default 100; 0 disables)
inside the append's own batch, so an anchor never describes a chain
that rolled back. A periodic job can call `anchor_statement` to anchor
the current head on demand.

## Append-only triggers

`audit_event` refuses `UPDATE` and `DELETE` at the database level, in
both dialects: `RAISE(ABORT, 'audit_event is append-only (docs/PLAN.md
§3 rule 6)')` on SQLite and a plpgsql function on Postgres. There is
no escape hatch — a correction is a new event, never a rewrite.

## Test vectors

All fixtures are invented; company `01ARZ3NDEKTSV4RRFFQ69G5FAV`.

### A — the genesis event (seq 1, person actor)

    {"actor":{"kind":"person","person_id":"usr_01HQZX9K3M7VN2P4R6T8W0Y1B3"},"approval_id":null,"at":"2026-01-15T09:30:00Z","command":"company.open","company_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV","id":"01HQZX9K3M7VN2P4R6T8W0Y1B3D","input_hash":"44eeef65813ca84bd050857c05139f161ba75b21816d5dbb4f6f342175966f9e","objects":["01ARZ3NDEKTSV4RRFFQ69G5FAV"],"outcome":"allowed","reason":null,"request_id":"req_01HQZX9K3M7VN2P4R6T8W0Y1B4","seq":1}

`prev_hash` `"0"` — `4273717302a68e320cf231985697b4717785ca684bfa446db2be4db93314ff55`

### B — seq 2, agent actor, approval, escapes and non-ASCII

`prev_hash` = A's hash. The reason carries a quote, a backslash and a
newline; `Müller` stays literal.

    {"actor":{"key_id":"key_01HQZX9K3M7VN2P4R6T8W0Y1B3","kind":"agent","tier":"post-within-limits"},"approval_id":"apr_01HQZX9K3M7VN2P4R6T8W0Y1B9","at":"2026-01-15T10:05:00Z","command":"journal.post_entry","company_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV","id":"01HQZX9K3M7VN2P4R6T8W0Y1B3E","input_hash":"1217aedc9d46506d6615d3eee94ba2cd4ad5f49d87a9f4ae2ce1caade150d2a8","objects":["01HQZX9K3M7VN2P4R6T8W0Y1B3F","01HQZX9K3M7VN2P4R6T8W0Y1B3G"],"outcome":"needs-approval","reason":"above the daily limit for \"Müller GmbH\"; line 2\\3\nreview","request_id":"req_01HQZX9K3M7VN2P4R6T8W0Y1B5","seq":2}

`e7fcfb60c92dd9bd057e45b954f92252d5b559df5c30414701171ca243bb22f8`

### C — a nested command input, for `input_hash`

Keys sort at every depth; the negative amount is a plain integer.

    {"amount_minor":-12500,"currency":"EUR","document":{"number":"INV-2026-0042","supplier":"Müller GmbH"},"lines":[{"account":"1010","minor":12500},{"account":"1400","minor":-12500}]}

`input_hash` = `6a5a5e99838616339141c885e1db91f770320cd8b3eaded4389d5a7146e9e608`