# 0005: Embed `cratefield-auth-passkeys` in the Ledgers server

Date: 2026-10-21. Status: Accepted (people sign-in superseded by 0008).

## Context

Ledgers needs passkey-based authentication for person approvals (high-risk actions,
period close, etc.). The harness provides `cratefield-auth-passkeys`, but it is not
yet published to crates.io; it exists at a pinned git `rev` in the Cratefield
repository.

The question: should we run a separate Cratefield auth service (dedicated instance,
shared hardening, independent release cadence) or embed `cratefield-auth-passkeys`
into the Ledgers server process?

## Decision

**Embed `cratefield-auth-passkeys` into the Ledgers server** via one pinned git
`rev` in `[patch.crates-io]` per ADR 0001.

When it reaches crates.io, update the workspace pin to the published version and
remove the patch.

## Consequences

### Self-host simplicity (wins)
- One process, one database, one deployment unit
- No cross-service session handoff or token exchange
- Works offline; no network dependency on a separate auth service
- Simpler deployment and operational surface (one service to secure, upgrade, monitor)

### Reduced reuse (trades)
- A separate auth service with independent hardening and release cadence is not
  available for other services (upside for factor-zero infrastructure; tradeoff for
  Ledgers alone).
- If Ledgers later needs to share passkey data with other services (e.g., sharing
  the same person across multiple products), a separate auth service becomes
  valuable; we would need to extract it then.

## Implementation notes

- The open build has one owner: recovery is one-time recovery codes (shown once,
  hashed at rest) plus `ledgers admin recover-owner` requiring shell access,
  audited.
- Person sessions use short-lived httpOnly SameSite=Strict cookies set in the
  server response; no separate session service.
- CLI login uses the published `cratefield-module-device-auth` (RFC 8628) approved
  in the app via passkey so the CLI never sees the credential.
- Agent keys use the harness `ApiKeys` with namespace `lk`, verified on every
  request (no caching).
- The `IdentityProvider` trait in `ledgers-auth` is the extension point: the cloud
  can implement custom SSO there.
- Passkeys wiring waits until the git `rev` is pinned in Cargo.toml (not done in
  issue #12).
- Device-auth and CLI wiring land with the server and CLI builds (later issues).
