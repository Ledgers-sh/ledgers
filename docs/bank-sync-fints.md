# FinTS Bank Synchronization

**Free for your own accounts, Germany only, self-hosted.**

Ledgers can sync transactions and balances from German banks via FinTS PIN/TAN
if you self-host. This is **not** a replacement for licensed account aggregators.

## Features

- **Read-only**: Balances and statements only. No payment initiation.
- **PIN handling**: Prompted per sync (optional: keychain). Never logged.
- **TAN**: User-controlled; never automated.
- **Amount precision**: Exact minor units, never floats.
- **Idempotent**: Re-syncing yields identical transaction IDs; merge on `external_id`.

## Tested Banks

**Status: No real-bank testing yet.** Sandbox testing and DKW product registration
are pending human-gated setup.

## Limitations

- **Germany only**: FinTS is a German standard.
- **Self-hosted only**: Multi-tenant requires licensed aggregators (PSD2 compliance).
- **MT940 only**: CAMT.053 not yet supported by fints-rs.
- **Booked transactions only**: Pending transactions are skipped.

## PIN and Security

- **Default**: Prompted per sync. Nothing stored.
- **With keychain**: Enable feature `keychain` to use system keyring.
- **Redacted**: `SecurePin` redacts all output (`Debug` and `Display` → `****`).
  No trace in logs or errors.

## Configuration

```
bank_code: "10010010"  # BLZ (8 digits)
user_id: "your_login"
iban: "DE89..."
bic: "COBADEDXXX"  # Optional
product_id: "your-registered-dkw-id"
pin_source: "prompt"  # or "keychain" (feature)
```

## Re-sync and Deduplication

Re-syncing the same or overlapping date ranges produces identical `external_id`s:

```
Sync 1: 2026-01-01 to 2026-01-31 → {A, B}
Sync 2: 2026-01-20 to 2026-02-10 → {A, B, C}
Merged by external_id: {A, B, C} (no duplicates)
```
