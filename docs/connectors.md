# Connected sources

Connected sources import what a vendor charges (invoices, cost, or usage) as
draft bills, split across business units. The framework is in
`crates/ledgers-connect`; the connectors are `cloudflare`, `anthropic`,
`planetscale` (earlier releases), `aws` and `vercel`, `gcp`, `azure` and
`supabase`, `openai` and `openrouter`, `stripe` and `polar`, `spaceship` (domains, renewals), `twilio`, `meta_ads`, `google_ads` and `jira`. Supabase, Spaceship and Jira book no money;
OpenRouter books one invoice per day with no allocation; Stripe posts journal entries
(not bills); the others book bills by month.

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

## OpenAI (`openai`): needs an admin key

The organization costs API, `GET https://api.openai.com/v1/organization/costs`, with
`start_time` and `end_time` (Unix seconds), `bucket_width=1d`, `group_by=project_id`,
paged by `page` while `has_more`. Each bucket's results carry `amount` (`value` in
dollars, `currency` in lower case) and `project_id` (null when unattributed).
Source: <https://platform.openai.com/docs/api-reference/usage/costs>

- **Credential.** An **admin API key**, created by an organization owner or admin under
  Settings, Organization, Admin API keys. A project key cannot read organization costs.
- **Allocation key.** The project. A cost with no project books as `unattributed`.
- **Months.** A month is booked once it has ended, as for the other cost connectors.
- **Not verified live.** The costs reference page could not be checked against a live
  admin key when this was written. If OpenAI answers 404 for a valid admin key, the
  sync is recorded as a failed run with that status, and nothing is booked.

## OpenRouter (`openrouter`)

The activity endpoint, `GET https://openrouter.ai/api/v1/activity?date=YYYY-MM-DD`, one
request per UTC day. Each row has `usage` (dollars, a number), `requests`, `model`,
`provider_name` and `endpoint_id`. It needs a **management key** (created at
<https://openrouter.ai/settings/management-keys>); a regular key is refused.
Source: <https://openrouter.ai/docs/api/api-reference/analytics/get-user-activity>

- **What is booked.** Each completed UTC day in the window is one invoice,
  `openrouter-usage:{date}:USD`, booked once and skipped on later syncs. Days with no
  spend book nothing.
- **Retention limit.** The endpoint keeps only the last 30 completed days, so a whole
  month is never in reach. That is why the books are per day, not per month.
- **Allocation.** Not yet. Allocation by model needs usage weights per day, and the
  framework keeps usage per month. OpenRouter days therefore write no usage rows and
  book to the connection's default unit, with a note saying so. Per-model totals per day
  appear in the run notes.
- **Test connection.** Today is not a completed day, so the test reads yesterday's
  activity to prove the key, and books nothing.

## Stripe (`stripe`): balance transactions as journal entries

Stripe is a payment source, not a cost source. Its books are journal entries, not
bills. The source of truth is the **balance transactions**:
`GET https://api.stripe.com/v1/balance_transactions` with `limit=100`, `created[gte]`
and `created[lt]` for the window, `expand[]=data.source` (so each row carries its
charge or refund), paged by `starting_after` while `has_more`. Source:
<https://docs.stripe.com/api/balance_transactions/list>

Each balance transaction with amount `A`, fee `F` and net `N = A - F` becomes one
balanced entry in its **settlement currency** (the balance transaction's currency):

| reporting category | revenue account | fee expense | clearing account |
|---|---|---|---|
| `charge` | `A` credit | `F` debit | `N` debit |
| `refund`, `dispute` | `A` (reverses revenue) | `F` debit | `N` (credit when negative) |
| `fee` | none | `A` (the fee charged) | `A` credit |
| anything else | suspense account | `F` debit | `N` debit |

`payout` posts **no entry here**. A payout moves money from the clearing account to
the bank, and it is booked when its bank line is matched, through the bank rule
(`bank.transactions.post`). The clearing account reaches zero when the payout's bank
side is booked, so a charge, then a refund, then a payout leaves it at zero. The
server test `stripe_ledger.rs` checks exactly this, through the bank module's own
suggestion and posting path.

- **Presentment.** The presentment currency and amount (from `data.source`) go in the
  entry's description, for example `presented as USD 11000 (minor units)`. No FX line
  is booked: the books are in the settlement currency only.
- **Allocation.** The line's resource is the charge's `product` (or `unit`) metadata. The
  connection's allocation rules map it to a business unit; otherwise the default unit.
- **Idempotency.** Each entry's id is `stripe-bt:{balance transaction id}`. The ledger
  refuses a second entry with the same external id, the sync checks before it posts,
  and the table `connector_ledger_line` records the link. A repeat sync posts nothing.
- **Postings.** Entries are posted one at a time through `journal.post_entry` (exposed as
  `entries.post`), because each posting numbers its entry from the counter it reads.
  Refunds are their own entries, not reversals of the charge, because a Stripe refund is
  a new event. `entries.transfer` is not used.
- **Unknown categories.** Anything not in the table goes to the suspense account, with a
  note in the run. It is never dropped. Each suspense entry also raises an item in the
  Approvals inbox (`connections.suspense_rebook`). Its preview shows the transaction, the
  amount, why it is in suspense, and the proposed account (the connection's revenue
  account). A person approves it, and the approval posts a balanced entry that takes the
  amount off suspense and books it to that account. Rejecting it leaves the amount in
  suspense. Each entry raises at most one item; a rejected item is not raised again. To book
  to a different account, reject the item and change the connection's revenue account, or
  book the entry by hand: the approval freezes its target, as every approval does.
- **Connect.** Set `stripe_account` to a connected account id (`acct_…`) to read that
  account's balance transactions with the `Stripe-Account` header. A connected account is
  its own allocation resource: a charge with no `product` metadata allocates by the account
  id, so an allocation rule such as `acct_1Example` → a business unit maps each connected
  account to its unit. Create one connection per connected account to keep the books
  separate. Leave the field empty for the platform's own account.
- **Limits and throttling.** A window of more than 20,000 transactions is read up to that
  limit, with a note; a later sync reads the rest. A `429` is retried with backoff, up to
  three attempts, then reported as a failed run.

**Credential.** A **restricted API key** with read access only to Balance, Charges,
Refunds, Payouts, Disputes and Invoices, and no other resource. For Connect, grant the
key's Connect permission only when it reads connected accounts.

**Config.** `clearing_account` (the Stripe balance, a postable asset),
`revenue_account` (charges and their reversals), `suspense_account` (unknown
categories), `expense_account` (fees), and the optional `stripe_account`. Each account is
checked against the chart when the connection is saved: it must exist, not be archived,
and not be a heading.

**Suggested bank rule.** When the connection is created, Ledgers proposes a bank rule:
a payout is an inflow whose description contains `stripe` (amount at least 1 minor unit),
which books to the clearing account. The rule is proposed by the connector and waits in
Bank, Rules until a person enables it.

## Polar (`polar`): orders and refunds into a Polar balance

Polar is a merchant of record. Its books are journal entries, as for Stripe. Source:
<https://polar.sh/docs/api-reference/orders/list> (orders) and
<https://polar.sh/docs/api-reference/refunds/list> (refunds).

- **Credential.** An **organization access token** with `orders:read` and `refunds:read`.
  Nothing else.
- **Orders.** `GET https://api.polar.sh/v1/orders/`, paged by `page` (`limit=100`) while
  `pagination.max_page` allows, in the window by `created_at`. Each order with total `T`,
  tax `X` and platform fee `F` posts to the **Polar balance** (`clearing_account`): revenue
  `T - X` (or `T` when no tax account is set), the fee to `expense_account` as a debit, and
  `T - F` debited to the Polar balance. Tax goes to `tax_account` when one is set.
- **Refunds.** `GET https://api.polar.sh/v1/refunds/` reverses them: revenue (and tax) are
  debited, and the Polar balance is credited with the refunded amount.
- **Payouts.** Polar's public API has no payout or balance endpoint, so payouts are not read.
  A payout reaches the bank as a bank line. A suggested bank rule (contains `polar`, inflows)
  books it to the Polar balance when a person enables it, and the balance reaches zero. The
  server test `polar_ledger.rs` runs this path: an order, a refund and the payout leave the
  balance at zero.
- **Tax.** With no `tax_account`, tax stays inside revenue and the run notes say so.
- **Allocation.** An order's product (`product_id`) is its resource; a refund has no product
  of its own, so it allocates to the default unit.
- **Idempotency.** Entries are `polar-order:{id}` and `polar-refund:{id}`. A repeat sync
  posts nothing.
- **Config.** `clearing_account` (the Polar balance), `revenue_account`, `expense_account`
  (fees), and the optional `tax_account`. Each is checked against the chart on save.

## Spaceship (`spaceship`): domains and renewals, no bills

Spaceship is the registrar for the Factory Zero domains. Its public API is at
<https://docs.spaceship.dev> (base `https://spaceship.dev/api/v1`). Researched for this
connector:

- **Authentication.** Two headers on every request: `X-Api-Key` and `X-Api-Secret`
  (the docs' samples write them as `X-API-Key` and `X-API-Secret`; header names are
  case-insensitive).
- **Scopes.** The API lists `domains:read`, `domains:write`, `domains:transfer`,
  `domains:billing`, `contacts:read`/`write`, `dnsrecords:read`/`write`,
  `asyncoperations:read`, `sellerhub:read`/`write` and `hyperlift:*`. Ledgers asks for
  **`domains:read` only**, which is all the domain list needs.
- **Billing: none.** The reference has **no billing, invoice, order or transaction
  endpoint** for the account. `domains:billing` is a scope on register, renew, restore
  and transfer (writes), not a read. The only "transactions" are SellerHub SafePay
  escrow sales of domains you sell (`GET /v1/sellerhub/safepay-transactions`), which are
  income, not a cost, and need `sellerhub:read`. They are not read.
- **Domain list.** `GET /v1/domains?take=1..100&skip=N&orderBy=name` returns
  `items` and `total`. Each item has `name`, `unicodeName`, `isPremium`, `autoRenew`,
  `registrationDate`, `expirationDate`, `lifecycleStatus`, `verificationStatus`,
  `eppStatuses`, `suspensions`, `privacyProtection`, `nameservers` and `contacts`.
- **Pricing.** The list has **no price**. Prices appear only on the availability
  endpoints (`premiumPricing`), for a name not yet registered. So the renewal price of a
  domain you already hold is not available from the API.
- **Async and errors.** Long operations return 202 with `spaceship-async-operationid`;
  `GET /v1/async-operations/{id}` gives `pending`, `success` or `failed`. Ledgers reads
  only the domain list, so no async operation is involved.

**What Ledgers does with this.** A sync reads the domain list (paged by `take` and `skip`,
up to 5,000 domains) and books **nothing**: no bill, no usage row, no journal entry. Each
domain is kept as metadata in the run, and the **renewals view** lists the domains that
expire within 60 days, plus those already past expiry, soonest first, with the expiry
date and auto-renew status. The view reads the registrar live when it is opened; nothing
is stored. Renewal amounts are not shown, because the API gives none. Actual renewal
charges come from the bank feed or the invoice inbox, and the renewal reminders do not
replace them.

- **Credential.** An API key with `domains:read`, created for this account only. The
  form asks for the key and the secret as one credential object, sealed with
  `LEDGERS_CONNECT_KEY`.
- **Config.** None. Expense account and default unit are required by the form as for every
  connection, but nothing is posted to them.
- **Allocation.** None. Nothing is split, so no allocation rule applies. The domains carry
  their names, so a domain-name rule (for example `squeez.in` to the Sqeezin unit) is a
  reasonable mapping to set up when a cost source exists.
- **Dates.** `expirationDate` is read as its first ten characters (`YYYY-MM-DD`). The
  exact format of a live answer has not been checked against an account; check the first
  live list before relying on the renewal dates.
- **Tests.** `crates/ledgers-connect/tests/spaceship.rs` runs the connector against recorded
  answers: headers, the query, paging by `skip`, parsing, idempotent re-sync, the renewals
  window, 401 and 403 (not retried), 429 and 5xx (three attempts), and a missing field.

## Twilio (`twilio`): usage costs, one bill per month and currency

The Usage Records API, `GET https://api.twilio.com/2010-04-01/Accounts/{AccountSid}/Usage/Records/Monthly.json`
with `StartDate` and `EndDate` (`YYYY-MM-DD`), `PageSize=1000` and `IncludeSubaccounts=false`.
Each record is one usage category in one month: `category`, `price` (a decimal string),
`price_unit` (the currency Twilio bills in, usually `USD`) and `start_date`. Paging follows
`next_page_uri`, which must stay on `api.twilio.com`. Sources:
<https://www.twilio.com/docs/usage/api/usage-record> and
<https://www.twilio.com/docs/iam/api/account>.

- **Credential.** An **API key** SID (`SK…`) and its secret, sent as HTTP Basic (the key SID
  as user name, the secret as password). Use a standard or restricted API key. **Never the
  main auth token.**
- **Config.** `account_sid` (required, `AC` and 32 hex characters) and `subaccount_sid`
  (optional). One connection per account. A subaccount needs its own connection, so its
  cost is not counted twice. Usage is read with `IncludeSubaccounts=false`.
- **What is booked.** One invoice per month and currency, `twilio-cost:{YYYY-MM}:{currency}`,
  whose total is the sum of the categories. The invoice description lists each category's
  amount in minor units. The aggregate `totalprice` category is skipped, since the
  categories add up to it. The sign of a price is ignored: the amount is booked by its size,
  because the docs do not say whether a charge is reported as a negative price. The current month is not booked; re-syncing a month is a no-op.
- **Allocation.** A usage row's resource is the account (or subaccount) SID. An allocation
  rule keyed by that SID maps the connection's costs to a business unit. Without a rule,
  costs book to the connection's default unit and are listed in the mapping queue. Usage
  rows carry no venture, so map by subaccount, not by phone number.
- **Balance.** `GET /2010-04-01/Accounts/{AccountSid}/Balance.json` is read each sync and
  shown as a run note, labelled "information only, not booked". The balance is read from
  the parent account, because a child account has no balance. The docs do not say whether
  an API key may read it, so a refusal is a note, not a failed sync. **Check the first live
  sync for this note.**
- **Errors.** 401 and 403 are refusals and are not retried. 429 and 5xx are retried three
  times, then reported. A next-page link that leaves `api.twilio.com` is refused.
- **Tests.** `crates/ledgers-connect/tests/twilio.rs`: Basic auth and the query, following
  `next_page_uri`, one invoice per month and currency with the categories in the
  description, the current month skipped, a repeat sync, a subaccount scope, the balance as
  a note, 401, 403, 429 and 5xx, and account SID validation.
- **Not verified live.** The balance endpoint's fields (`account_sid`, `balance`,
  `currency`) come from a third-party reference, since the Twilio page could not be
  fetched; check them against the first live answer.

## Meta Ads (`meta_ads`): monthly ad spend per campaign

The Marketing API insights edge of one ad account:
`GET https://graph.facebook.com/v26.0/act_{ad_account_id}/insights` with `level=campaign`,
`time_increment=monthly`, `time_range={"since":...,"until":...}` and the fields `spend`,
`account_currency`, `campaign_id`, `campaign_name`, `date_start` and `date_stop`. Paging
follows `paging.next`, which must stay on `graph.facebook.com` under the pinned version.
The Graph API version is pinned in `GRAPH_VERSION` (`v26.0`, the version the Meta
documentation shows as current when this was written). Sources:
<https://developers.facebook.com/docs/marketing-api/insights>.

- **Credential.** One system-user access token with the `ads_read` permission only,
  sent as `Authorization: Bearer`. Never a token with `ads_management` or a business-wide
  admin permission. The token is never put in a URL.
- **Config.** `ad_account_id` (required: `act_` and its digits; one connection per ad account)
  and `name_prefixes` (optional, see Allocation).
- **What is booked.** One invoice per month and account currency, `meta-ads-cost:{YYYY-MM}:{currency}`,
  unique per connection. Each campaign is one usage line for its month (the allocation key is
  the campaign id). The current month is not booked. Re-importing a month is a no-op.
- **Allocation.** Allocation rules by campaign id first. Then `name_prefixes`: pairs of
  `PREFIX:UNIT` separated by commas (for example `SQZ:SQZ`). A campaign whose name starts
  with the prefix, followed by a separator or the end, books to that unit when no rule names
  it. The rule applies to this sync's split; the mapping list does not change. The form
  suggests the unit codes you already use.
- **Errors.** 401 and 403 are refusals (the token or its `ads_read` permission), not retried.
  429 and 5xx are retried three times with backoff (500 ms, then 1 s), then reported.
  Graph also throttles some apps with HTTP 400 and an error code in the body; the connector
  does not read that body, so such a throttle is reported as a status 400 and is not retried.
- **Tests.** `crates/ledgers-connect/tests/meta_ads.rs`: the pinned URL and paging, Bearer
  auth, per-month invoices and campaign lines in minor units, the current month skipped,
  a repeat import, 401, 403, 429 with backoff, 5xx, and the account id check.

## Google Ads (`google_ads`): monthly campaign cost per campaign

The Google Ads API `searchStream` for one customer, with this GAQL:
`SELECT campaign.id, campaign.name, customer.currency_code, segments.month, metrics.cost_micros FROM campaign WHERE segments.date BETWEEN 'since' AND 'until'`.
The API version is pinned in `API_VERSION` (`v26`, the latest release in the Google Ads release
notes when this was written). Sources:
<https://developers.google.com/google-ads/api/docs/query/overview> and
<https://developers.google.com/google-ads/api/docs/release-notes>.

- **Credentials.** A developer token, an OAuth client ID and secret, and a refresh token
  granted the `https://www.googleapis.com/auth/adwords` scope. The refresh token is exchanged
  at `oauth2.googleapis.com/token` on each sync; the access token is never stored.
- **Developer token.** A production account needs an **approved developer token (basic
  access)**. A test-level token works straight away, against test accounts only, and its
  data is test data.
- **Config.** `customer_id` (required, ten digits, dashes allowed), `login_customer_id`
  (optional, the manager (MCC) account that the customer is reached through) and
  `name_prefixes` (optional, as for Meta Ads).
- **Money.** `metrics.cost_micros` is an integer in millionths of the account currency. It
  is read as an exact integer and rounded half up to the currency's minor units; no float is
  used.
- **What is booked.** One invoice per month and currency, `google-ads-cost:{YYYY-MM}:{currency}`,
  unique per connection. Each campaign is one usage line. The current month is not booked.
- **Errors.** 400, 401 and 403 from the token endpoint, and 401 and 403 from the search, are
  refusals and are not retried. 429 and 5xx are retried three times with backoff, then reported.
- **Tests.** `crates/ledgers-connect/tests/google_ads.rs`: the token exchange and the search
  headers (`developer-token`, `login-customer-id`), the GAQL window, exact micro conversion,
  the current month skipped, a repeat import, refusals, 429 with backoff, 5xx and the customer id check.

## Jira (`jira`): seats and projects, a planned cost, nothing booked

**Research (October 2026).** Atlassian's Commerce REST API has invoice endpoints
(`/v2/invoices`, `/v1/invoices/{id}/download`). They are addressed by a transaction account
(`X-transaction-account`), the commerce account of a Marketplace vendor, and the docs do not
say they return a customer's own Jira Cloud subscription invoices, so they are not used. The
Organization (admin) REST API's user endpoints list users and roles; the parts of it checked
showed no price or invoice data. Atlassian therefore exposes no billing data this connector can
book, as far as the documentation checked shows. Sources:
<https://developer.atlassian.com/platform/commerce/rest/api-group-invoices/> and
<https://developer.atlassian.com/cloud/admin/organization/rest/api-group-users/>.

**The connector books nothing.** It reads, per site (`{site}.atlassian.net`):

- `GET /rest/api/3/users/search` (paged): the active accounts of type `atlassian`. These are
  the **seats in use**.
- `GET /rest/api/3/applicationrole`: each product with its licensed seats and user count.
  Needs Jira administrator rights; without them the product counts are a note, not a failure.
- `GET /rest/api/3/project/search` (paged): the projects and their keys. A project key is the
  allocation key, so a rule maps the project to a business unit.

- **Credentials.** The account email and an **API token**, sent as HTTP Basic. Least
  privilege: an account that can browse users and projects. Jira administrator rights are
  optional. **OAuth 2.0 (3LO) is not implemented.** If it is added, it needs read-only scopes
  only; the exact scope names are to be checked against Atlassian's scope list first.
- **Config.** `site` (required, `example.atlassian.net`; only `*.atlassian.net` hosts are
  allowed, and the requests go to that host alone), `price_per_seat` (optional, a decimal
  amount you type) and `price_currency` (an ISO code, required with a price).
- **Planned cost.** Seats in use times the price per seat, shown as a note on the connection
  ("Planned monthly cost ... not booked"). **It is a plan, never an expense.** Actual charges
  come from the bank feed or the invoice inbox.
- **What is stored.** Usage rows for each project, for the current month, with weight 1 each.
  They are the allocation keys for the planned cost. A split of the planned cost by unit
  is not yet shown anywhere in the app.
- **Tests.** `crates/ledgers-connect/tests/jira.rs`: the site allow-list, Basic auth to the
  site only, seat counting, product and project notes, no invoice booked, the planned note
  from the typed price, refusals, 429 with backoff, 5xx, and a repeat sync.

## Providers not built, and why

- **MiniMax.** No billing or usage API is documented. Balance and usage are on the
  console pages only, so there is nothing to read.
- **Z.AI (GLM).** No billing or usage query API is documented (the docs point to the
  console). Undocumented endpoints in community tools are not used.
- **Mistral.** The admin usage endpoint (`/v1/admin/usage`) exists, but it is Enterprise
  only and in preview, and it reports per workspace, with no split by key. It is left out
  until it is generally available for the plans Ledgers is used on.
- **Google Gemini.** The Gemini API has no spend API. Gemini costs appear in the Cloud
  Billing export, so they come through the **Google Cloud** connector (`gcp`), filtered
  to the Gemini API service. No separate Gemini connector is needed.
- **Other providers researched** (DeepSeek, Groq, Cohere, Perplexity, xAI, Together,
  Fireworks): the research is kept in `docs/connectors/llm-providers.md`. Each is left
  for a later slice; the reasons are recorded there.

## Open item: Supabase add-on prices

Supabase add-on prices carry an amount and an interval but no currency. Nothing is
booked in a currency until a live account's add-on response has been checked for a
currency field. Until then, Supabase reports these prices as run notes only.

## Deploy notes

- Migration `0002` widens the `connector` check to include `aws` and `vercel`;
  migration `0003` adds `gcp`, `azure` and `supabase`; migration `0004` adds `openai`
  and `openrouter`; migration `0005` adds `stripe` and the ledger idempotency table
  `connector_ledger_line` (D1 mirror `0044`); migration `0006` adds `polar` (D1 mirror
  `0045`); migration `0007` adds `spaceship` and `twilio` (D1 mirror `0050`).
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
