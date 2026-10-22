# 0008: People sign-in through a per-app auth instance, with the API as the session holder

Date: 2026-10-22. Status: accepted. Supersedes the people-sign-in part of
[0005](0005-passkeys-embedded.md) (the passkey verifier is not embedded in the
server). Part of #12.

## Context

Approvals need a trusted person (docs/PLAN.md section 5), and people need to
sign in to the web app. #12 asked for a decision between embedding
`cratefield-auth-passkeys` in the server (ADR 0005) and running a per-app
Cratefield auth instance, and for the trade-off to be recorded.

A per-app instance is being provided at `https://auth-staging.ledgers.sh` and
`https://auth.ledgers.sh`. It is an OpenID Connect provider: passkeys first,
magic-link email for first sign-in and recovery, ES256 ID and access tokens,
a JWKS, and a discovery document. The `roles` claim arrives for clients
registered to receive it.

## Decision

1. **The Ledgers API is a public OIDC client (authorization code with PKCE)**
   of the per-app instance. There is no client secret, so nothing secret ships
   in the Worker beyond `HARNESS_SECRET`, which seals the short-lived login
   cookie.
2. **The API holds the session (backend for frontend).** The browser never
   receives an OIDC token. `GET /v1/auth/login` starts the flow;
   `GET /v1/auth/callback` exchanges the code and verifies the ID token;
   the session is an opaque random id in an httpOnly, Secure cookie on the API
   host. The database keeps only its SHA-256 (`people_session`).
3. **ID tokens are verified in `ledgers-auth::oidc`**: ES256 only, a `kid` that
   the discovered JWKS publishes, the signature, `iss`, `aud` (and `azp` when
   there are several audiences), `exp` and `iat` with 60 s skew, `nonce`, and a
   non-empty `sub`. An `at+jwt` is refused as an ID token.
   `cratefield-auth-client` is not used for this: it accepts only access tokens
   (`typ: at+jwt`) and has no nonce check, so it cannot verify the callback's ID
   token. It stays the choice for verifying access tokens on other routes.
4. **Owner rule.** A person may use the company when their `roles` contain
   `owner` or `admin`. Any other account, including one with no roles, gets 403
   with a clear message on `/v1/auth/session` and on every command. The rule is
   secure by default: the first verified person is not made owner. The open build
   has one owner, so the owner role is the normal case. Roles are read at sign-in
   and held for the session (at most 12 hours), so a revoked role takes effect
   when the session ends or on the next sign-in.
5. **Sessions.** Absolute lifetime 12 hours from sign-in; idle lifetime 30
   minutes, slid forward on use once half spent and never past the absolute
   lifetime; logout deletes the row. Expired rows are swept on sign-in.
6. **Cookie.** `__Host-ledgers-session`: HttpOnly, Secure, `SameSite=Lax`,
   `Path=/`. The sign-in cookie `__Host-ledgers-login` (state, nonce, PKCE
   verifier, ten minutes) is HMAC-sealed with a key derived from
   `HARNESS_SECRET`. `SameSite=Lax` rather than the Strict named in #12: the callback is a top-level redirect from the issuer, which can
   sit on another site, and Strict would drop the sign-in cookie there.
7. **CSRF by Origin.** A request with the session cookie must name an allowed
   app origin (`LEDGERS_CORS_ORIGINS`) when it changes state; a read may omit
   Origin but a named one must be allowed. Cross-site pages cannot forge the
   Origin. A custom header was considered and not used: the harness's CORS layer
   does not allow custom headers, and the Worker's preflight answer would be the
   only place to add one.
8. **Agent keys are unchanged.** `Authorization: Bearer` wins over a cookie and
   takes the existing path, including the rate limit. Persons are never limited.

## Where the routes live

The harness mounts a module's routes only under `/v1/<name>`, and root paths
need a core change. The sign-in therefore lives at `/v1/auth/...`
(`/v1/auth/login`, `/v1/auth/callback`, `/v1/auth/logout`, `/v1/auth/session`).
The module is named `auth`, which the domain reservation list already holds.
The issuer must register `https://<api-host>/v1/auth/callback`.

## CORS and the native runtime

The Worker answers the credentialed preflight itself, and marks `/v1` responses
with `Access-Control-Allow-Credentials` for the configured app origins. The
harness's CORS layer cannot do this, and the native runtime builds its router
internally, so the native server cannot add it. Native dev uses the Vite proxy
(`app/vite.config.ts`) so the browser sees one origin.

## Consequences

- Self-hosters without an issuer keep working: sign-in answers 503
  (`sign-in-not-configured`), and the Advanced agent-key connect remains. A
  self-hosted owner who wants passkey sign-in needs an issuer instance. That
  reverses the "self-host without the hosted auth service" line in #12, and this
  is the trade-off accepted here.
- The native runtime refuses loopback and private destinations for outbound
  requests, so a local issuer on `127.0.0.1` cannot be used with
  `ledgers-server`. Local sign-in tests use `auth-staging` (see docs/deploy.md).
- Owner recovery (recovery codes, `ledgers admin recover-owner`) is not in this
  change. The issuer's email link is the recovery path for the person.
- Step-up approvals (passkey assertion bound to a request hash) remain open;
  they need the issuer's `acr`/`amr` claims, which this change does not read.
