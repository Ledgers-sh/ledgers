# Threat model: Ledgers M1

This document covers the agent-safe core (M1) before bills, extraction, or payments exist. It identifies assets, actors, trust boundaries, and defences against STRIDE threats.

## Assets

- **Books:** immutable ledger, period locks, numbering
- **Money-out authority:** agent key limits, per-entry and per-day caps; person payouts
- **Personal data:** company name, user identity (owner), approval audit
- **Keys:** agent API keys; passkey credentials for high-risk approvals

## Actors

- Malicious vendor (via document in M2)
- Compromised agent (stolen or leaked key)
- Compromised MCP client (untrusted LLM)
- Insider (owner with full access)

## Trust boundaries

```
┌─────────────────────────────────────────────────────┐
│  Ledgers Server (SQLite/Postgres + audit chain)     │
│  • Policy engine (tiers, limits, risk flags)        │
│  • Approval service (passkeys)                      │
│  • Rate limiter per agent key (native: SQLite-based;│
│    Workers: Cloudflare Rate Limiting)               │
│  • Hash-chained audit log (append-only)             │
└──────┬──────────────────────┬──────────────────┬────┘
       │                      │                  │
       ↓ /v1 (auth + rate)    ↓ MCP (stdio)      ↓ CLI
    HTTP                   LLM/Agent          Person
     TLS                    (untrusted)       (console)
```

## STRIDE per surface

### Document intake / extraction (M2 planned, signals only in M1)

| Threat | Surface | Status |
|--------|---------|--------|
| Injection | LLM prompt with document text as data only; fixed system instruction (crates/ledgers-policy/src/intake.rs) | Implemented |
| Hidden text | OCR text or embedded directives in PDFs | Residual risk |
| Model mis-extraction | Decimal amounts parsed exactly (decimal-string field); schema validation enforced | Implemented |
| Duplicate bill | Hash-based detection (screen.rs `DuplicateDocument`); same vendor + number + amount (`DuplicateBill`) | Planned M2 (signals in M1 via `screen()`) |

### MCP surface

| Threat | Surface | Status |
|--------|---------|--------|
| Injection | Document text escaped (mcp.rs `escape_document_text()`); untrusted-data rule in tool schema | Implemented |
| Replay | Approval cards non-replayable: hashed and passkey-bound | Verified existing (cratefield-auth-passkeys) |
| Spoofing | Hardcoded "From MCP" origin in audit; no agent approval authority | Verified existing |

### HTTP /v1 routes & CLI

| Threat | Surface | Status |
|--------|---------|--------|
| Spoofing | API keys with agent ID; key expiry and revocation checks | Implemented (decide.rs lines 139–148) |
| Tampering | Hmac or request signing | Not yet wired (planned M2) |
| Repudiation | Every command in audit with principal and timestamp | Verified existing (ledgers-audit) |
| Elevation | Policy enforced server-side, never delegated to agent/MCP/CLI | Implemented (decide.rs `PolicyChange` deny; decide.rs check order 1–8) |
| DoS | Rate limiter per agent key (rate_limiting.rs); fail closed (429) | Implemented: native via SQLite (ports().rate_limiter); Workers via Cloudflare Rate Limiting binding |
| Disclosure | Document text never logged (only hash); approval preview is frozen JSON | Implemented |

### Approval cards & passkey step-up

| Threat | Surface | Status |
|--------|---------|--------|
| Tampering | Card hash bound to passkey proof; proof verified before approval | Verified existing (cratefield-auth-passkeys) |
| Spoofing | Card shows amounts, accounts, and (when present) payee bank + changed-flag warning | Implemented (mcp.rs, service.rs `build_card`) |
| Phishing | Owner is sole approver (M1); no URL shortening | Verified existing |

### Policy & keys

| Threat | Surface | Status |
|--------|---------|--------|
| Elevation | Agents cannot change policy (PolicyChange always denied); humans can | Implemented (decide.rs line 165–172) |
| Resource exhaustion | Per-day limits enforced as guard statements (ledgers-db guard patterns) | Verified existing |
| Weak authentication | Keys expiry and revocation; tier-based tiers (Read, Draft, PostWithinLimits); per-currency limits | Implemented |

### Storage & audit

| Threat | Surface | Status |
|--------|---------|--------|
| Repudiation | Hash-chained audit log; sequence uniqueness enforced as guard statement | Implemented (ledgers-audit, ledgers-db) |
| Disclosure | Document hash only (no plaintext text storage in M1); audit chain available for offline verification | Implemented |

## Defences: signals vs. enforcement

**Risk flags** generated in `screen()` (screen.rs) are *signals for humans*, not blocks:
- `BankDetailsChanged` — payee bank differs from vendor history → card warns + passkey escalation
- `NewVendor` — no prior history → approval required
- `DuplicateBill` / `DuplicateDocument` — likely error or fraud → approval required
- `VolumeSpike` — agent's bill count jumped → approval required
- `RoundAmountUnderLimit` — whole 100-unit amount below limit → weak signal (not a blocker)
- `InstructionText` — suspicious NLP signals in document text → approval required; text itself is never instructions

**Enforcement**: `decide()` routes flagged documents to approval *before* posting; JournalPreview does not yet carry risk flags, so in M1 they are computed but not included in the decision path. For M2, flags will feed into decide.rs check #8 (lines 239–248).

## Residual risks

- **Vendor history**: a vendor's *first ever bill* carries no prior bank details → `NewVendor` flag only; insider must vet the IBAN
- **OCR / hidden text**: PDFs may contain embedded directives or OCR ambiguities; mitigation is human review
- **Model mis-extraction**: the LLM may hallucinate fields; mitigation is exact decimal-string parsing and schema validation
- **Insider**: owner has full approval authority and can read all data; no cryptographic boundary between persons and keys
- **Passkey compromise**: if credential keys leak, high-risk approvals can be forged; recovery is to revoke keys and require a reset

## How to add a corpus case

Add a test in `tests/adversarial_corpus.rs` (being written in M1):

1. Create an `ExtractionTest` in `tests/corpus/` with a PDF or text file (e.g., `injection_attempt.pdf`)
2. Add an expected result: `{ "expected_flags": ["InstructionText"], "expected_extraction": {...} }`
3. Reference it in the corpus runner and assert the flags and fields match
4. Mocked `TextModel::complete_as()` uses harness `structured_output`, so schema validation is real
5. Each case runs under a single test in CI; corpus grows without test explosion

---

*This document is a snapshot of M1 (agent-safe core). It will be updated as M2 (documents & bills), M3 (complete books) and M5 (security review) proceed. Do not treat statements about "planned for M2" as decisions; they are design notes.*
