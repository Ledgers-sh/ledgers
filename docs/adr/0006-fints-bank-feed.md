# 0006: FinTS bank feed via fints-rs 0.2.0

Date: 2026-10-22. Status: accepted.

## Context

Ledgers needs bank transaction and balance sync for import and reconciliation.
German banks support FinTS 3.0 (PIN/TAN) natively. The `fints-rs` crate
(floffel/fints-rs, crates.io, MIT) provides a pure-Rust client with:
- Typestate `Dialog<S>` for auth flow
- PIN and TAN support (PIN/TAN, decoupled pushTAN)
- MT940 parsing, `rust_decimal` amounts (exact, no floats)
- Reqwest 0.12 + rustls, tokio async, HTTP transport only
- Young project (~6 months, but MIT and stable API)

## Decision

1. **Depend on fints-rs 0.2.0 pinned exactly** (`=0.2.0` in workspace deps).
   Fork only if a real bug blocks self-hosting.

2. **Native/self-host only**: ledgers-fints is NOT a dependency of ledgers-server
   (which checks wasm32). For hosted tenancy, use licensed aggregators.

3. **Read-only**: balance and statement fetch only. No payment APIs.

4. **PIN handling**: never stored by default. `PinSource` trait abstracts:
   - `DefaultPinSource`: prompt per sync (no storage)
   - `KeychainPinSource` (feature `keychain`): system keyring, pure-Rust backends

5. **TAN stays with user**: decoupled TAN (pushTAN) is polled client-side.
   Two-step TAN via callback; never automated. Flow polls internally in confirm_and_fetch.

6. **Product ID gated**: read `option_env!("LEDGERS_FINTS_PRODUCT_ID")`; fail if absent
   and not in config. Registration with Die Deutsche Kreditwirtschaft is human-gated.

7. **Stable external_id**: SHA256 hash of (IBAN, booking_date, valuta_date, amount,
   currency, counterparty IBAN, purpose) + occurrence index. Format: `fints:<hash>:<index>`.
   Enables idempotent re-sync deduplication.

8. **Skip pending transactions**: only `Booked` status imported.

## Consequences

- Product registration is required before production use.
- Real-bank testing (Sparkasse, direct banks) is pending.
- No claims of production readiness until tested.
- Transport abstraction not needed; fints-rs HTTP is sufficient for self-hosting.
