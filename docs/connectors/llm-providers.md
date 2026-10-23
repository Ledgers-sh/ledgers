# LLM provider cost and usage APIs

Research for the LLM connectors (issue #142 framework, LLM slice). Question per provider:

- **(a) Invoices** — is there an official API that lists or returns invoices?
- **(b) Cost** — is there an official API that returns dollars by day or month?
- **(c) Per key / project / workspace** — can usage or cost be split by API key, project or workspace?

Sources are official provider docs unless marked otherwise. Checked 2026-10-23. Items marked **unverified** were only seen in third-party sources or search snippets and must be confirmed before implementation.

## Summary table

| Provider | (a) Invoices API | (b) Cost by day/month | (c) Split by key / project / workspace | Auth for reads | Read-only key? | Granularity / limits | Verdict |
|---|---|---|---|---|---|---|---|
| **OpenAI** | None found in official docs (a third-party blog claims `/v1/organization/invoices`; **unverified**) | Yes: `GET /v1/organization/costs`, `bucket_width=1d`, `group_by` incl. `project_id`, `line_item` | Yes: costs by `project_id`; `GET /v1/organization/usage/completions` with `group_by` incl. `api_key_id`/`project_id`/`model` (filters `api_key_ids`, `project_ids`) | Admin key (Bearer) | No read-only admin key documented | Costs: daily buckets only; usage: 1m/1h/1d; paginated via `next_page` | **Connect** |
| **Anthropic** | None | Yes: `GET /v1/organizations/cost_report`, 1d buckets, `group_by` = `description`, `workspace_id` only | Cost: by workspace only. Usage: `GET /v1/organizations/usage_report/messages` `group_by` incl. `api_key_id`, `workspace_id`, `model` | Admin key | No | Usage 1m/1h/1d; `limit` max 31 days / 168 h / 1440 min | **Connect** (slice 1; cost by workspace, usage by key) |
| **Google Gemini API (AI Studio)** | None | No API. Cost only in Cloud Billing console | No per-key breakdown; spend cap is per project (AI Studio) | — | — | — | **Invoice import only** for AI Studio keys |
| **Google Vertex AI** | None documented in Cloud Billing REST | Not via REST. Official route is the Cloud Billing **BigQuery export** (detailed usage cost table): `cost`, `credits`, `project.id`, hourly rows | Yes: by `project.id` and labels (labels only from when applied) | BigQuery read access to the export dataset (service account) | Yes (BigQuery Data Viewer on the dataset) | Hourly rows; "within a day, can take more than 24 hours" | **Connect** via BigQuery export (project = allocation unit) |
| **xAI** | Yes: `GET /v1/billing/teams/{team_id}/invoices` on management API (**confirmed in docs summary, verify path**) | Yes: `POST /v1/billing/teams/{team_id}/usage`, `timeUnit` day/hour etc., measure `usd`. Console Usage Explorer also shows USD | Per API key in console ("Group by API key"); **groupBy field name for API key in the REST API unverified** | Management key (`management-api.x.ai`), needs Management Keys read permission | Management key scopes not documented in fetched text | Time range `YYYY-MM-DD HH:MM:SS`, exclusive end | **Connect** once groupBy key field verified; else invoice-only |
| **Mistral** | No invoices endpoint in fetched admin docs | Yes: `GET https://api.mistral.ai/v1/admin/usage?month&year&workspace_id`, amounts by category, currency | Per workspace (not per key). Per-key breakdown not documented | Admin API key (`x-api-key`), **Enterprise plans only, Preview** | No | Monthly only | **Connect** (Enterprise only; workspace = allocation unit) |
| **DeepSeek** | None | None. `GET /user/balance` only (balance, not spend) | None | Not documented in page | — | — | **Invoice import only** |
| **Z.ai (GLM)** | None documented | None documented. Undocumented `/api/monitor/usage/*` endpoints appear in a community plugin only (**unverified**) | None documented | — | — | — | **Invoice import only** |
| **MiniMax** | None | None programmatic. Console "Usage" pages; CLI `mmx quota` (not an API) | None documented | — | — | — | **Invoice import only** |
| **OpenRouter** | Not documented in fetched pages | Yes: `GET /api/v1/activity?date=YYYY-MM-DD` (management key), per day, `usage` USD, `byok_usage_inference`; last 30 completed UTC days | Yes: per key via `api_key_hash` filter; per workspace via `group_by=workspace`. `GET /api/v1/key` returns the calling key's `usage`, `usage_daily`, `usage_weekly`, `usage_monthly`, `limit` with a **regular key** | Regular key for `/key` (self only); management key for `/activity` | Yes for `/key` (no management key needed) | Activity: one UTC day per request; no rate limit documented | **Connect** |
| **Groq** | None. Invoices in console only (Settings → Billing); usage page in console | No API. Feature request for billing API exists in community forum | Spend limits are org-wide, no per-key data | — | — | — | **Invoice import only** |
| **Together AI** | None documented | Yes (beta): `GET https://api.together.ai/v1/billing/usage?month=YYYY-MM&granularity=day|hour`, USD `cost` per line item, cursor pagination. Beta: enabled per organization by Together support, otherwise 404 | Not as rollup. `attributes` on inference line items carry `api_key_id` and `project_id` | Any project API key can read org usage (per docs); `organization_id` required at GA | Yes (any key) | Current month up to 1 h behind; prior months up to 24 h; `limit` 1–1000 | **Connect** (beta; gated) |
| **Fireworks AI** | None found (`/billing/summary` lists rated dollar line items by category) | Yes: `POST /v1/accounts/{account_id}/usageCosts:query` (dollars, `groupBy` incl. `API_KEY`, `DAY`, `MODEL`). `GET /v1/accounts/{account_id}/billing/summary` (dollars, daily buckets, categories) | Yes: `API_KEY` group. `usageCosts:query` with `ACCOUNT` scope requires account administrator; `SELF` scope returns only the caller | Fireworks API key (admin for account-wide cost) | Usage quantities endpoint (`billingUsage`) works with any key on the account but has no dollars | Max 31-day window per request | **Connect** (admin key) |
| **Cohere** | None found | None documented (per-response `meta.billed_units`; dashboard Billing and Usage page) | None documented | — | — | — | **Invoice import only** |
| **Perplexity** | None found | None documented (per-response usage cost is third-party only, **unverified**) | None documented | — | — | — | **Invoice import only** |
| **Cerebras, Hyperbolic etc.** | Not in scope of this list | — | — | — | — | — | Out of scope |

## Per-provider notes

**OpenAI.** `GET https://api.openai.com/v1/organization/costs` (admin key) returns dollar buckets; supported `group_by` includes `project_id` and `line_item`, and `bucket_width` is `1d` only. `GET https://api.openai.com/v1/organization/usage/completions` returns token usage with `group_by` on `api_key_id`, `project_id`, `model`; `bucket_width` `1m|1h|1d`. Without `group_by`, `project_id`/`model` come back `null`. The official reference pages were partially unreachable during research (403/404) so exact `group_by` lists must be re-checked against the live reference at implementation time. Sources: https://developers.openai.com/cookbook/examples/completions_usage_api ; https://community.openai.com/t/cost-api-project-ids-filter-not-working/1387697 (inconsistency report).

**Anthropic.** Cost report: https://platform.claude.com/docs/en/api/beta/organization/cost_report/retrieve — `amount` is a decimal string in **lowest currency units (cents)**. Usage by key: https://platform.claude.com/docs/en/api/beta/organization/usage_report/retrieve_messages — `group_by[]=api_key_id` works. Cost is not split by key; usage is.

**Google (Gemini API and Vertex AI).** No REST API returns cost by project or key. Official route: Cloud Billing export to BigQuery, detailed usage table, which has `project.id`, labels and `cost`/`credits` (https://docs.cloud.google.com/billing/docs/how-to/export-data-bigquery-tables/detailed-usage). Gemini API spend is in the Cloud Billing console under the "Gemini API" service (https://ai.google.dev/gemini-api/docs/billing). AI Studio usage dashboards are not per key. Spend caps are per project (experimental). Consequence: connector reads a BigQuery dataset with a read-only service account; allocation unit = GCP project.

**xAI.** Management API base `https://management-api.x.ai`, auth via management key with Management Keys read permission (https://docs.x.ai/developers/rest-api-reference/management). Billing (`/v1/billing/teams/{team_id}/...`) exposes invoices (`GET .../invoices`), usage analytics (`POST .../usage`, USD, time buckets) and prepaid balance (https://docs.x.ai/developers/rest-api-reference/management/billing). Console Usage Explorer groups by API key (https://docs.x.ai/docs/key-information/usage-explorer). The REST `groupBy` key for API key was not visible in fetched text; **verify before implementing per-key**. The endpoints also mutate state (POST top-up, payment method), so the connector must call only the GET/usage read paths.

**Mistral.** Admin API (Enterprise, Preview) base `https://api.mistral.ai/v1/admin`, `x-api-key` with an Admin API key (not a standard key). `GET /usage?month=&year=&workspace_id=` gives billing-period consumption by category with currency (https://docs.mistral.ai/admin/admin-api/usage-metrics). No per-key breakdown documented. Spend limits at `/spend-limit`. Invoices are console-only in docs.

**OpenRouter.** Two relevant reads: (1) `GET https://openrouter.ai/api/v1/key` with the calling key returns its own `usage` and period usage (https://openrouter.ai/docs/api/api-reference/api-keys/get-current-key) — works with a plain key, so a per-key read-only connection is possible; (2) `GET https://openrouter.ai/api/v1/activity?date=YYYY-MM-DD&api_key_hash=&group_by=workspace` requires a **management key**, returns per-model/provider rows for the last 30 completed UTC days with `usage` (USD) and `byok_usage_inference` (https://openrouter.ai/docs/api/api-reference/analytics/get-user-activity). Activity data before workspace resolution is attributed to the account default workspace and cannot be backfilled.

**Together AI.** `GET https://api.together.ai/v1/billing/usage` (https://docs.together.ai/reference/billing-usage): USD `cost` per line item, daily or hourly windows, `after` cursor. Beta, enabled per organization by support, otherwise 404. No per-key rollup; `api_key_id` available in `attributes` of inference line items, so per-key totals require summing. Billing docs: https://docs.together.ai/docs/billing.

**Fireworks AI.** Dollar costs: `POST /v1/accounts/{account_id}/usageCosts:query` grouped by `API_KEY` (https://docs.fireworks.ai/accounts/exporting-usage-and-costs). `billingUsage` gives quantities only, "No dollars". The page is internally inconsistent on whether per-key dollars exist; the `usageCosts:query` text says `API_KEY` is a valid grouping, so **verify with a live call on the account before implementing**. Admin-only for `ACCOUNT` scope.

**Providers with no cost or per-key API (invoice import only):**
- **DeepSeek** — only `GET /user/balance` (https://api-docs.deepseek.com/api/get-user-balance). No usage, cost or invoice endpoint in the API doc set.
- **Z.ai (GLM)** — no official billing or usage API in docs (https://docs.z.ai/devpack/usage-policy.md points to console). Undocumented `/api/monitor/usage/*` endpoints are used by a community plugin only; not implemented.
- **MiniMax** — console pages for usage and invoices; `mmx quota` CLI. No API documented (https://platform.minimax.io/docs/m-plan/usage-rules.md; https://platform.minimax.io/docs/llms.txt lists no billing endpoints).
- **Groq** — invoices and usage in console (https://console.groq.com/docs/billing-faqs). Spend limits are org-wide. Billing API is a community feature request (https://community.groq.com/t/how-do-i-get-an-invoice-for-my-groq-usage/66).
- **Cohere** — per-response `meta.billed_units` only; billing via dashboard (https://docs.cohere.com/docs/how-does-cohere-pricing-work).
- **Perplexity** — API reference has no usage or billing endpoints (https://docs.perplexity.ai/docs/getting-started/overview).

## Sources

- OpenAI cookbook, Usage API: https://developers.openai.com/cookbook/examples/completions_usage_api
- Anthropic cost report: https://platform.claude.com/docs/en/api/beta/organization/cost_report/retrieve
- Anthropic usage report: https://platform.claude.com/docs/en/api/beta/organization/usage_report/retrieve_messages
- Google BigQuery detailed usage export: https://docs.cloud.google.com/billing/docs/how-to/export-data-bigquery-tables/detailed-usage
- Google Gemini API billing: https://ai.google.dev/gemini-api/docs/billing
- xAI management API: https://docs.x.ai/developers/rest-api-reference/management
- xAI management billing: https://docs.x.ai/developers/rest-api-reference/management/billing
- xAI Usage Explorer: https://docs.x.ai/docs/key-information/usage-explorer
- Mistral admin usage metrics: https://docs.mistral.ai/admin/admin-api/usage-metrics
- DeepSeek balance: https://api-docs.deepseek.com/api/get-user-balance
- Z.ai usage policy: https://docs.z.ai/devpack/usage-policy.md
- MiniMax usage rules: https://platform.minimax.io/docs/m-plan/usage-rules.md
- MiniMax API overview: https://platform.minimax.io/docs/api-reference/api-overview
- OpenRouter current key: https://openrouter.ai/docs/api/api-reference/api-keys/get-current-key
- OpenRouter activity: https://openrouter.ai/docs/api/api-reference/analytics/get-user-activity
- Groq billing FAQ: https://console.groq.com/docs/billing-faqs
- Together billing usage: https://docs.together.ai/reference/billing-usage
- Together billing: https://docs.together.ai/docs/billing
- Fireworks usage and cost export: https://docs.fireworks.ai/accounts/exporting-usage-and-costs
- Cohere pricing: https://docs.cohere.com/docs/how-does-cohere-pricing-work
- Perplexity overview: https://docs.perplexity.ai/docs/getting-started/overview

Not verified against official docs (fetch blocked or only in secondary sources): OpenAI invoices endpoint, OpenAI costs reference page (403/404 during research), Together `api_key_id` rollup, Fireworks per-key dollars, xAI API-key groupBy field, Perplexity per-response cost.
