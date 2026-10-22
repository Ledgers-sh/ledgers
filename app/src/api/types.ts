/**
 * Wire types for the Ledgers HTTP API.
 *
 * Source of truth: the command registry (crates/ledgers-commands/src/registry.rs)
 * for paths, methods and tiers, and the serializable DTOs in
 * crates/ledgers-ledger/src/surface.rs and the report structs in
 * crates/ledgers-ledger/src/report.rs for field names.
 *
 * Several command input/output schemas are still empty in the registry
 * (`serde_json::json!({})`), so the shapes marked "inferred" below are
 * hand-written from the domain types. They are to be replaced by the
 * generated client (harness `client-ts`) once the OpenAPI document carries
 * real schemas.
 */

/** Integer minor units as a decimal string, e.g. "1234" for 12.34 EUR. */
export interface Amount {
  minor: string;
  currency: string;
}

/** Shared by entries.post and the entry lines returned by entries.list / entries.get. */
export type Side = 'debit' | 'credit';

export interface LineInput {
  account_number: string;
  side: Side;
  base: Amount;
  fx: null;
  tax_code: null;
  dimensions: Record<string, string>;
  memo: string | null;
}

export interface EntryDraftInput {
  journal_id: string;
  document_date: string;
  posting_date: string;
  tax_point_date: string | null;
  base_currency: string;
  description: string;
  source_documents: string[];
  external_id: string | null;
  source_app: string | null;
  reverses: string | null;
  lines: LineInput[];
}

/** Body of POST /v1/entries (PostEntryInput in ledgers-ledger/src/surface.rs). */
export interface PostEntryInput {
  journal_id: string;
  fiscal_year: number;
  entry: EntryDraftInput;
}

/** Output of POST /v1/entries (PostEntryOutput, SerializablePostedEntry). */
export interface PostedEntry {
  entry_id: string;
  journal_id: string;
  fiscal_year: number;
  number: number;
  recorded_at: string;
}

export interface PostEntryOutput {
  entry: PostedEntry;
}

/** Body of POST /v1/entries/{external_id}/reverse (inferred). */
export interface ReverseEntryInput {
  posting_date: string;
  description: string;
}

/** A posted entry as returned by entries.list and entries.get (inferred). */
export interface EntryRecord {
  external_id: string | null;
  entry_id: string;
  journal_id: string;
  fiscal_year: number;
  number: number;
  document_date: string;
  posting_date: string;
  base_currency: string;
  description: string;
  /** Who posted it: an agent key or a person. Drives the agent/human colour. */
  posted_by: 'agent' | 'person';
  reverses: string | null;
  reversed_by: string | null;
  recorded_at: string;
  lines: EntryLineRecord[];
}

export interface EntryLineRecord {
  account_number: string;
  side: Side;
  base: Amount;
  memo: string | null;
}

export interface EntriesListOutput {
  entries: EntryRecord[];
}

/** Account as returned by accounts.list (SerializableAccountEntry, ledgers-ledger surface.rs). */
export interface Account {
  number: string;
  name_key: string;
  account_type: string;
  normal_balance: string;
  currency: string | null;
  parent: string | null;
  tags: string[];
  is_postable: boolean;
}

export interface AccountsListOutput {
  accounts: Account[];
}

/** Trial balance row (TrialBalanceRow, ledgers-ledger/src/report.rs). */
export interface TrialBalanceRow {
  account_code: string;
  debit: Amount;
  credit: Amount;
  balance: Amount;
}

/** Output of reports.trial_balance (inferred envelope around the rows). */
export interface TrialBalanceOutput {
  period: string;
  rows: TrialBalanceRow[];
}

/** Output of reports.profit_and_loss (ProfitAndLoss). */
export interface ProfitAndLossOutput {
  period: string;
  revenue: Amount;
  expenses: Amount;
  net_income: Amount;
}

/** Output of reports.balance_sheet (BalanceSheet). */
export interface BalanceSheetOutput {
  as_of: string;
  assets: Amount;
  liabilities: Amount;
  equity: Amount;
}

/** One row of reports.account_balance_by_dimension (DimensionBalance). */
export interface DimensionBalanceRow {
  dimension_value: string;
  balance: Amount;
}

export interface AccountBalanceByDimensionOutput {
  rows: DimensionBalanceRow[];
}

/** bills.match is not in the registry yet; the response is shown as returned. */
export type BillsMatchOutput = Record<string, unknown>;

/** One currency's limits on an agent key, as canonical major-unit strings ("1000.00"). */
export interface AgentKeyLimit {
  per_entry: string;
  per_day: string;
}

/** An agent key as `keys.list` and `keys.create` return it. Never carries the secret. */
export interface AgentKeyView {
  id: string;
  name: string;
  /** The public prefix, `lk_live_<id>`. */
  prefix: string | null;
  tier: 'read' | 'draft' | 'post-within-limits' | string;
  limits: Record<string, AgentKeyLimit>;
  created_at: string;
  created_by: string;
  last_used_at: string | null;
  revoked_at: string | null;
}

/** GET /v1/keys (keys.list). */
export interface KeysListOutput {
  keys: AgentKeyView[];
}

/** POST /v1/keys (keys.create). */
export interface KeysCreateInput {
  name: string;
  tier: string;
  limits: Record<string, AgentKeyLimit>;
}

/** The creation answer: the key, and its full token, which is shown once. */
export interface KeysCreateOutput {
  key: AgentKeyView;
  token: string;
}

/** POST /v1/keys/{id}/revoke (keys.revoke). */
export interface KeysRevokeOutput {
  key: AgentKeyView;
}

