# Connected sources

Connected sources import what a vendor charges (invoices, cost, or usage) as
draft bills, split across business units. The framework is in
`crates/ledgers-connect`; the connectors are `cloudflare`, `anthropic`,
`planetscale` (earlier releases) and, from this release, `aws` and `vercel`.

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

## Deploy notes

- Migration `0002` widens the `connector` check to include `aws` and `vercel`.
  SQLite cannot alter a check constraint, so the migration rebuilds
  `connector_connection` with foreign keys off. It is guarded so a repeat is
  harmless. A crash between its DROP and RENAME needs the rename run by hand.
- The D1 mirror (`crates/ledgers-server/d1/migrations/0041_ledgers_connect_connect_providers.sql`)
  carries the same SQL. D1 may not accept `PRAGMA foreign_keys`; run it against a
  staging D1 database before production.
- `LEDGERS_CONNECT_KEY` must be set on the server for any connection to hold a
  credential.
