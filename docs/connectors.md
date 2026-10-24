# Connected sources

Connected sources import what a vendor charges (invoices, cost, or usage) as
draft bills, split across business units. The framework is in
`crates/ledgers-connect`; the connectors are `cloudflare`, `anthropic`,
`planetscale` (earlier releases), `aws` and `vercel`, and `gcp`, `azure` and
`supabase`. Supabase books no money; the others book bills and usage.

## How a sync books money

- **Credentials.** A connector with one field takes a `token`. A connector
  with several takes a `credential` object holding exactly its fields. The
  server seals the credential with `LEDGERS_CONNECT_KEY`, bound to the
  connection id. The token is never returned, never logged and never echoed
  in an error. A refused credential names the field, never the value.
- **Test connection.** `connections.test` (in the app, the Test connection
  button) reads the vendor for today with the stored credential and reports
  whether it answered, with a sentence when it did not. It writes nothing: no
  run, no bill, no usage.
- **Sync window.** The first sync reads three calendar months. Vendors whose
  totals move until the month closes (AWS, Vercel, Anthropic) book only months
  that have ended, so a month is imported once, complete.
- **Idempotent import.** Each invoice has a stable external id (for cost
  connectors `aws-cost:{period}:{currency}`). A repeated sync of the same id
  creates nothing; usage snapshots are replaced for each period read.
- **Allocation.** Usage rows are the allocation key. A usage row's `quantity`
  is its weight, and the invoice total splits across units in proportion to
  the weight of each unit's resources (largest remainder, exact). A resource
  with no allocation rule books to the connection's default unit and is listed
  in the mapping queue, so nothing is silently lost to the wrong unit.
- **Several currencies in one period.** Usage weights are kept per period, not per
  currency. A period whose costs are in more than one currency therefore writes no
  weights, and its invoices book to the default unit with a note, instead of letting
  one currency's amounts distort another's split.
- **Currency.** Amounts are read as exact decimals and summed at six decimals,
  then rounded half up once to the currency's minor units. Any ISO 4217 code
  Ledgers knows is booked in that currency; an unknown code is not booked and
  a note says so.
- **Errors.** A 401 or 403 is "refused the token", not retried. A 429 or 5xx is
  retried up to three attempts with backoff, then reported. A timeout or
  connection failure is retried the same way. A malformed answer is reported
  with the field it failed on. A failed sync is recorded as a failed run.

## AWS (`aws`)

Cost Explorer `GetCostAndUsage`, daily, metric `UnblendedCost`, grouped by
`SERVICE` and, with `tag_key`, by that tag. Source:
<https://docs.aws.amazon.com/aws-cost-management/latest/APIReference/API_GetCostAndUsage.html>

**Create this IAM user and nothing else.** Cost Explorer does not support
resource-level permissions, so the resource is `*`:

```json
{
  "Version": "2012-10-17",
  "Statement": [
    { "Effect": "Allow", "Action": "ce:GetCostAndUsage", "Resource": "*" }
  ]
}
```

Create one access key for that user and enter the key ID and the secret. No
other permission is needed or should be granted.

- **Config.** `tag_key` (optional): allocate by this tag's value instead of by
  service. A cost with no value books to the resource `untagged`.
- **Allocation keys.** `service` (the AWS service name) or `tag` (the tag value).
- **Signing.** AWS Signature Version 4, written in `crates/ledgers-connect/src/sigv4.rs`
  so the crate builds for wasm32 without the AWS SDK. The signer is checked
  against the AWS signature test suite's `get-vanilla` vector.
- **Throttling.** Cost Explorer can answer a throttled request with HTTP 400 and
  a `ThrottlingException` body. The connector does not retry a 400, so a
  throttled sync is a failed run to rerun later; it never books a partial month.

## Vercel (`vercel`)

The FOCUS billing charges, `GET https://api.vercel.com/v1/billing/charges`
with `from` and `to` as ISO 8601 UTC. The answer is newline-delimited JSON,
one FOCUS 1.3 charge per line. Source:
<https://vercel.com/docs/rest-api/reference/endpoints/billing/list-focus-billing-charges>

- **Token.** A personal access token of a team member with the **Billing**
  (or Enterprise Viewer) role. The REST API documents no narrower token scope,
  so this is the least privilege available today.
- **Config.** `team_id` (optional): the team to bill, such as `team_abc123`.
  Leave it empty for a personal account.
- **What is booked.** `Usage`, `Tax`, `Adjustment` and `Credit` charges, in
  `BillingCurrency`. A `Purchase` is prepaid credit bought; it is cash, not an
  expense, so it is counted in a note and not booked.
- **Allocation key.** The project, from `Tags.ProjectId`. A charge with no
  project tag books to `unattributed`.
- **Invoices.** Vercel invoices are not in the REST API, so no invoice is
  imported. The charges are the source of truth.
- **Not yet verified against a live account.** The schema documents `BilledCost`
  as a number; the reader accepts a number or a decimal string. Check the first
  live answer before relying on it.

## Google Cloud (`gcp`)

The Cloud Billing API cannot return costs, so the cost comes from the **Cloud
Billing export to BigQuery** (the standard usage cost table). The connection names
the table: `project` (the project that runs the query), `dataset` and `table`.
Ledgers runs the query with the BigQuery REST `jobs.query` and reads the rows with
`getQueryResults`, following page tokens and waiting for a job that is still
running. Source: <https://cloud.google.com/bigquery/docs/reference/rest/v2/jobs/query>

- **Credential.** A service account JSON key. Create one key for a service account
  with **BigQuery Data Viewer** on the export dataset and **BigQuery Job User** on
  the project that runs the query, and paste the whole JSON file.
- **Auth.** Ledgers signs an RS256 JWT with the key (`ring`, built for wasm32 with
  `wasm32_unknown_unknown_js`) and exchanges it at `https://oauth2.googleapis.com/token`
  for a token with the `bigquery.readonly` scope. The key is never sent anywhere else.
- **What is booked.** Each month's net cost per project: `cost` plus `credits`,
  summed as NUMERIC (the export's `cost` is a FLOAT64, so each row is converted first).
- **Allocation key.** The project id. A row with no project books as `unattributed`.
- **Export lag.** The export lags usage by hours; only completed months are booked.

## Microsoft Azure (`azure`)

The Cost Management query at subscription scope,
`POST https://management.azure.com/subscriptions/{id}/providers/Microsoft.CostManagement/query?api-version=2025-03-01`,
one request per calendar month: `type: Usage` (actual cost), `granularity: None`,
`PreTaxCost` summed, grouped by `ResourceGroup`. The answer's columns are matched by
name and `properties.nextLink` pages. Source:
<https://learn.microsoft.com/rest/api/cost-management/query/usage>

- **Credential.** A client secret of an app registration with the **Cost Management
  Reader** role on the subscription (assign it on the subscription, not the tenant).
- **Config.** `tenant_id`, `client_id` and `subscription_id`, each a GUID.
- **Auth.** The Entra ID client-credentials flow at
  `https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token` with
  `scope=https://management.azure.com/.default`. The secret is percent-encoded in the
  form body and never logged.
- **Allocation key.** The resource group. A cost with no resource group books as
  `unattributed`.
- **Currency.** Read from the `Currency` column of each row.

## Supabase (`supabase`): metrics only, no bills

Supabase's Management API gives no cost in a currency for an organization, so this
connector **books nothing** and creates no bills or usage rows. Each sync reads the
organization's projects, then for each project (up to 25):

- the add-ons it pays for (`GET /v1/projects/{ref}/billing/addons`), with their
  price `amount` and `interval`. The API gives **no currency**, so these are shown
  in the run notes as "amount without currency, not booked", and are never converted
  or guessed; and
- the last day's request counts (`usage.api-counts?interval=1day`), summed in the
  run notes as counts, not money.

The notes appear in the run history of the Connected sources screen. Because nothing
is split, no allocation rule applies to Supabase. Source: the Management API's
OpenAPI document at <https://api.supabase.com/api/v1-json>.

- **Credential.** A personal access token with read access to the organization.
  Supabase documents scoped tokens (limited to chosen organizations and permissions);
  use one limited to the organization. The reference does not name the exact
  permissions, so no permission name is given here.
- **Config.** `organization`, the organization slug.
- **Limit.** Projects beyond the first 25 are counted in a note, not read.

## Deploy notes

- Migration `0002` widens the `connector` check to include `aws` and `vercel`;
  migration `0003` adds `gcp`, `azure` and `supabase`.
  SQLite cannot alter a check constraint, so the migration copies the rows into a
  new table and renames it into place. It defers foreign-key checks with
  `PRAGMA defer_foreign_keys` rather than switching them off, because D1 ignores
  `foreign_keys = OFF`. The file runs as one transaction, natively and on D1.
- The D1 mirror (`crates/ledgers-server/d1/migrations/0041_ledgers_connect_connect_providers.sql`)
  is the same SQL. `crates/ledgers-server/tests/d1_connect_rebuild.rs` applies it, as one
  batch in a transaction, to a database holding a connection, a credential and a run, and
  checks that the rows survive and the foreign-key check is empty.
- `LEDGERS_CONNECT_KEY` must be set on the server for any connection to hold a
  credential.
