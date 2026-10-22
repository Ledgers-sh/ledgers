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

1. `https://app-staging.ledgers.sh` loads and the Connect screen starts at
   `https://api-staging.ledgers.sh`.
2. Connect with the key from step 6. The overview loads and no Bills item
   appears in the navigation.
3. Post a balanced entry and check the trial balance shows it.

## Local development

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
