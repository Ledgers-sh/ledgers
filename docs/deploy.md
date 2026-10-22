# Deploying to Cloudflare staging

Staging is two deployments:

| Part | Where | Source | Name |
|---|---|---|---|
| Server | Worker, D1 `ledgers-staging`, R2 `ledgers-documents-staging` | `crates/ledgers-server` | `ledgers-api-staging` at `https://api-staging.ledgers.sh` |
| Web app | Pages project | `app/` | `ledgers-app-staging` at `https://app-staging.ledgers.sh` |

Nothing in CI deploys these. Each step below is run by hand, once, by someone
with access to the Cloudflare account. Production (`[env.production]`) is not
covered here.

Tools: Rust 1.98.1 (pinned in `rust-toolchain.toml`), `worker-build` 0.8.x,
Node 22, and wrangler through `npx`. Every wrangler call below pins
`wrangler@4.148.0`.

## 1. Create the storage (once)

```sh
npx wrangler@4.148.0 login
npx wrangler@4.148.0 d1 create ledgers-staging
npx wrangler@4.148.0 r2 bucket create ledgers-documents-staging
```

`d1 create` prints a `database_id`. Keep it for step 2.

## 2. Fill the placeholders

In `crates/ledgers-server/wrangler.toml`, under `[env.staging]`:

- `database_id = "STAGING_D1_ID"`: replace with the id from step 1.
- `LEDGERS_COMPANY_ID = "STAGING_COMPANY_ID"`: replace with a ULID you choose
  (for example from `ulid` or any ULID generator). It names the one company the
  Worker serves; it is an identifier, not a secret. The bootstrap in step 6
  creates the company under this id.

Commit the filled values with the change that deploys them. Neither is a
secret; the secrets are set in step 3 and never go in the repository.

## 3. Set the secrets

Run from `crates/ledgers-server`. Values go straight from `openssl` into
wrangler, so they are never printed or written to a file:

```sh
cd crates/ledgers-server
openssl rand -hex 32 | npx wrangler@4.148.0 secret put HARNESS_SECRET --env staging
openssl rand -hex 32 | npx wrangler@4.148.0 secret put BOOTSTRAP_TOKEN --env staging
```

`BOOTSTRAP_TOKEN` is needed for step 6 only. Keep the value in your password
manager until step 6 is done, then remove it (step 6 shows how).

Two more secrets are needed for documents and bills (issues #20 to #22). Neither
is set by the repository; the operator sets them the same way:

```sh
openssl rand -hex 32 | npx wrangler@4.148.0 secret put LEDGERS_DOCUMENT_SIGNING_KEY --env staging
npx wrangler@4.148.0 secret put LEDGERS_EXTRACT_API_KEY --env staging
npx wrangler@4.148.0 secret put LEDGERS_MINIMAX_API_KEY --env staging
```

- `LEDGERS_DOCUMENT_SIGNING_KEY` signs the five-minute download links for
  stored invoices. It is required outside `LEDGERS_DEV=1` (at least 32
  characters). Changing it ends download links in flight, nothing else.
- `LEDGERS_EXTRACT_API_KEY` is the Anthropic Messages API key the model
  extractor (`claude-haiku-4-5`) uses to read scans and photos. Without it only
  PDFs with a text layer are read, and the Bills screen says so. The key is
  never logged or returned by any endpoint.
- `LEDGERS_MINIMAX_API_KEY` is the MiniMax key for the MiniMax reader
  (`https://api.minimax.io/v1/chat/completions`). It reads images (JPEG, PNG,
  GIF, WEBP) and PDFs with a text layer. A scanned PDF without a text layer
  gets the "needs a photo" flag, because MiniMax's documented inputs do not
  include PDF files. The key must be an API key from the MiniMax platform; a
  coding-plan subscription on its own is not one.
- `LEDGERS_EXTRACT_PROVIDER` chooses the reader: `anthropic`, `minimax` or
  `rules`. Unset, it is `anthropic` when `LEDGERS_EXTRACT_API_KEY` is set,
  `minimax` when only `LEDGERS_MINIMAX_API_KEY` is set, and the rules otherwise.
  Naming a provider whose key is missing is a configuration error at startup,
  never a silent fallback. `LEDGERS_MINIMAX_MODEL` overrides the MiniMax model
  id (default `MiniMax-M3`).

Staging sets `LEDGERS_EXTRACT_PROVIDER = "minimax"` in `[env.staging.vars]`;
production sets no provider and keeps the default.

## 4. Apply the migrations

The migrations are the flat files in `crates/ledgers-server/d1/migrations/`.
They are the SQLite migrations of every module, in the order the native server
applies them. `cargo test -p ledgers-server --test d1_migrations` fails if they
drift from the modules; regenerate with
`LEDGERS_WRITE_D1_MIGRATIONS=1 cargo test -p ledgers-server --test d1_migrations`.

Check them against local D1 first, then apply to staging:

```sh
cd crates/ledgers-server
npx wrangler@4.148.0 d1 migrations apply ledgers-staging --env staging --local
npx wrangler@4.148.0 d1 migrations apply ledgers-staging --env staging --remote
```

Applying twice is safe: wrangler records each applied file.

## 5. Deploy the Worker

Dry run first. It builds the Worker and reports its bindings without uploading:

```sh
cd crates/ledgers-server
worker-build --release
npx wrangler@4.148.0 deploy --dry-run --env staging --outdir /tmp/lg-dry
npx wrangler@4.148.0 deploy --env staging
```

The Worker limits each agent key to 100 requests per 60 seconds through the
`RATE_LIMITER` binding declared under `[env.staging.ratelimits]`. No secret or
step is needed for it. Without the binding the Worker answers `/v1` with 503.

Attach `api-staging.ledgers.sh` to the Worker as a custom domain (Workers &
Routes, then Custom Domains) so the URL in `LEDGERS_PUBLIC_URL` answers. Doing
it in the dashboard keeps DNS changes out of the deploy.

Check the deployment:

```sh
curl -sS -o /dev/null -w '%{http_code}\n' https://api-staging.ledgers.sh/v1/openapi   # expect 200
```

## 5a. People sign-in (once per environment)

People sign in with a passkey through the per-app auth instance (ADR 0008).
The Worker needs its issuer, its client id, and the app's URL:

| Variable | Staging value | Where |
|---|---|---|
| `LEDGERS_AUTH_ISSUER` | `https://auth-staging.ledgers.sh` | `[env.staging.vars]`, set |
| `LEDGERS_AUTH_CLIENT_ID` | the id the issuer gives the Ledgers client | `[env.staging.vars]`, placeholder until registered |
| `LEDGERS_APP_URL` | `https://app-staging.ledgers.sh` | `[env.staging.vars]`, set |
| `LEDGERS_PUBLIC_URL` | `https://api-staging.ledgers.sh` | `[env.staging.vars]`, already set |
| `HARNESS_SECRET` | secret, step 3 | seals the login cookie; rotating it ends sign-ins in flight |

1. Register the client with the staging issuer as a public client (no secret),
   PKCE S256, with the redirect URI **`https://api-staging.ledgers.sh/v1/auth/callback`**,
   scopes `openid email`, and the `roles` claim enabled for it. Copy the client
   id into `LEDGERS_AUTH_CLIENT_ID` in `crates/ledgers-server/wrangler.toml`
   (replace `STAGING_AUTH_CLIENT_ID`) and commit with the deploy change.
2. Sign-in is off (the routes answer 503) while `LEDGERS_AUTH_ISSUER` is unset.
   The owner account needs the `owner` (or `admin`) role in that client, or the
   app shows "This account cannot use this ledger".
3. Migration `0018_auth_people_session.sql` creates `people_session`. It is
   applied in step 4 like the others.

Production gets its own client and redirect URI (`https://api.ledgers.sh/v1/auth/callback`)
when it is set up; nothing here covers it.

## 6. Bootstrap the company and its first key (once)

`POST /admin/bootstrap` creates the company (under `LEDGERS_COMPANY_ID`), imports
the generic SME chart, opens the general journal `general` and the current
fiscal year, and issues one full-tier key (post within limits). It answers
`201` with the key in the response body, and that is the only time the key is
shown. Nothing logs it.

The route works only while `BOOTSTRAP_TOKEN` is set and no agent key exists.
Any later call answers `409`, and a deployment without the secret answers
`404`.

Save the response to a private file, then copy the key into your password
manager:

```sh
umask 077
curl -sS -X POST https://api-staging.ledgers.sh/admin/bootstrap \
  -H "Authorization: Bearer $BOOTSTRAP_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"company":"Demo GmbH","currency":"EUR"}' \
  -o "$HOME/ledgers-staging-bootstrap.json" -w '%{http_code}\n'   # expect 201
# copy .api_key from that file into your password manager, then:
rm "$HOME/ledgers-staging-bootstrap.json"
```

Remove the bootstrap token so the route is closed for good:

```sh
npx wrangler@4.148.0 secret delete BOOTSTRAP_TOKEN --env staging
```

The key authenticates the API from now on:

```sh
curl -sS https://api-staging.ledgers.sh/v1/accounts -H "Authorization: Bearer $LEDGERS_KEY"
```

If the bootstrap fails partway (for example a storage error), run it again: it
creates only what is missing. A database that already has a key cannot be
bootstrapped again; resetting staging means deleting and recreating the D1
database, which needs an explicit decision because it drops the data.

## 7. Build and deploy the web app

The server origin is fixed at build time by `VITE_LEDGERS_API`. The Connect
screen still lets a user change it.

```sh
cd app
npm ci
npm run typecheck && npm test
VITE_LEDGERS_API=https://api-staging.ledgers.sh npm run build
npx wrangler@4.148.0 pages project create ledgers-app-staging   # once
npx wrangler@4.148.0 pages deploy dist --project-name ledgers-app-staging
```

`app/wrangler.jsonc` names the Pages project and its output directory (`dist`).
Attach `app-staging.ledgers.sh` to the Pages project in the dashboard. That
origin is the only one the staging Worker allows
(`LEDGERS_CORS_ORIGINS`).

The app shows Bills only when the server's OpenAPI document lists
`bills.match`. Until the server ships that command, Bills stays hidden.

## 8. Smoke check

1. `https://app-staging.ledgers.sh` loads on the sign-in screen, and the
   passkey link goes to `https://api-staging.ledgers.sh/v1/auth/login`.
2. Sign in with the owner account. The top bar shows the email, and
   `https://api-staging.ledgers.sh/v1/auth/session` answers 200 with the person
   (`curl` cannot do this without the cookie; use the browser).
3. Post a balanced entry and check the trial balance shows it. Sign out and
   confirm `/v1/auth/session` answers 401.
4. Agent keys still work: connect through "Advanced: connect with an agent key"
   with a key from step 6 and read the overview.

### Request timing (staging only)

`wrangler.toml` sets `LEDGERS_SERVER_TIMING = "1"` on the staging environment.
Domain command responses then carry a `Server-Timing` header:

```
server-timing: total;dur=13.00, db;dur=9.00, db-calls;dur=10, db-statements;dur=10
```

- `total` is the Worker's wall time for the request.
- `db` is the time spent in database calls, in milliseconds.
- `db-calls` and `db-statements` are counts carried in the `dur` slot:
  round trips to D1, and the SQL statements inside them (a batch counts once
  per statement).

Check it with `curl -sD - -o /dev/null -H "Authorization: Bearer $KEY" https://api-staging.ledgers.sh/v1/accounts/1030`.
Never set the variable on production; the header is for staging measurements.

## Local development

Sign-in on a laptop needs a publicly routable issuer. The native server
refuses loopback and private destinations for outbound calls, so a local issuer
on `127.0.0.1` does not work. Test with `auth-staging`:

1. Register `http://localhost:5173/v1/auth/callback` as an extra redirect URI of
   the staging client (a dev-only registration; keep it out of production).
2. Run the native server with the issuer and the dev origin:

```sh
cd crates/ledgers-server
LEDGERS_DEV=1 \
LEDGERS_PUBLIC_URL=http://localhost:5173 \
LEDGERS_APP_URL=http://localhost:5173 \
LEDGERS_AUTH_ISSUER=https://auth-staging.ledgers.sh \
LEDGERS_AUTH_CLIENT_ID=<the staging client id> \
cargo run -p ledgers-server
```

3. In `app/`, run `LEDGERS_DEV_API=http://127.0.0.1:3000 VITE_LEDGERS_API=http://localhost:5173 npm run dev`.
   Vite proxies `/v1` (including `/v1/auth`) to the server, so the browser sees
   one origin and the session cookie is first-party.

The dev key that `LEDGERS_DEV=1` prints still works through "Advanced: connect
with an agent key". `HARNESS_SECRET` is generated per run in dev, so a sign-in
started before a restart fails and must be started again.

`wrangler dev` runs the same Worker on your machine against a local D1:

```sh
cd crates/ledgers-server
npx wrangler@4.148.0 d1 migrations apply ledgers-staging --env staging --local
printf 'BOOTSTRAP_TOKEN=%s\n' "$(openssl rand -hex 32)" > .dev.vars   # gitignored
npx wrangler@4.148.0 dev --env staging --local --port 8787
```

`.dev.vars` is gitignored and must never be committed. Delete it when you are
done. For the native server (SQLite), `LEDGERS_DEV=1 cargo run -p ledgers-server`
creates the dev company and prints a dev key; that path has no bootstrap route.

## Demo company

The demo company Halden Works GmbH (`crates/ledgers-demo`, issue #51) is loaded
automatically by the native development server (`LEDGERS_DEV=1`) when its company has no
entries. It posts through the executor: agent entries with the bootstrap key's own bearer
token, and the owner's entries under a person principal, so each entry records whether an
agent or a person posted it. Dimension values are created with the store's domain function.
Re-running is a no-op: an entry whose `external_id` exists is skipped, and a reversal whose
target is already reversed is skipped.

### Seeding staging

Staging can load the same book with `POST /admin/seed-demo`, after the bootstrap in step 6.
The Worker runs the same loader as the development server. Agent entries post as an
internal agent principal that exists only in memory: no agent key is issued or stored for
the seed. Owner entries post as the person "Demo owner" through the executor. The route is
idempotent by `external_id`, so a second call posts nothing.

It refuses in this order, and reads or writes nothing before the checks pass:

- `404` while the `SEED_TOKEN` secret is unset,
- `401` for a missing or wrong bearer token,
- `403` unless the variable `LEDGERS_ALLOW_DEMO_SEED` is `"1"`. That variable is set in
  `[env.staging.vars]` only. Never set it in production.

The response and the Worker log carry counts only, never entry text or keys. Seed the
staging company like this:

```sh
export SEED_TOKEN="$(openssl rand -hex 32)"
printf '%s' "$SEED_TOKEN" | wrangler secret put SEED_TOKEN --env staging
curl -sS -X POST https://api-staging.ledgers.sh/admin/seed-demo \
  -H "Authorization: Bearer $SEED_TOKEN" -w '\n%{http_code}\n'   # expect 200 with counts
wrangler secret delete SEED_TOKEN --env staging
unset SEED_TOKEN
```

The one-liner `openssl rand -hex 32 | wrangler secret put SEED_TOKEN --env staging` sets
the secret, but the call needs the value, so keep the token in a shell variable as above.
Delete the secret as soon as the call has answered, so the route closes again. A second
call with the secret still set answers `200` with `"posted": 0`.

`LEDGERS_DEMO=0` keeps a development company empty.
