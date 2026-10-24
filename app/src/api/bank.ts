/**
 * The bank screens' HTTP calls (crates/ledgers-bank/src/model.rs for the
 * shapes, crates/ledgers-server/src/dispatch/bank.rs for the paths). The file
 * upload that precedes an import is the bills upload (POST /v1/documents/upload).
 * A failed call throws an ApiError whose `detail` is the server's own words.
 */
import { request, type ClientConfig } from './client';

export type Config = ClientConfig;

/** An amount: integer minor units as a string, and its ISO 4217 currency. */
export interface Amount {
  minor: string;
  currency: string;
}

export interface BalanceView {
  date: string;
  amount: Amount;
}

export interface AccountView {
  id: string;
  name: string;
  iban: string;
  bic: string | null;
  currency: string;
  ledger_account_number: string;
  opening: Amount;
  opening_date: string;
  csv_mapping: CsvMapping | null;
  archived: boolean;
  statement_balance: BalanceView | null;
  ledger_balance: Amount;
  difference: Amount;
  unmatched_count: number;
  matched_count: number;
  ignored_count: number;
  created_at: string;
  updated_at: string;
}

/** A CSV column mapping. A column is "#n" (1-based) or a header name; the app writes "#n". */
export interface CsvMapping {
  delimiter: string;
  skip_rows: number;
  has_header: boolean;
  decimal: string;
  date_format: string;
  booking_date: string;
  value_date: string | null;
  amount: string | null;
  debit: string | null;
  credit: string | null;
  currency: string | null;
  counterparty_name: string | null;
  counterparty_iban: string | null;
  remittance: string[];
  bank_reference: string | null;
}

export type SuggestionKind = 'rule' | 'bill' | 'entry';

export interface Suggestion {
  kind: SuggestionKind;
  target_id: string;
  /** 0 to 100. */
  score: number;
  /** Stable reason codes; the screen words the ones it knows. */
  reasons: string[];
  account_number: string | null;
}

export interface MatchedView {
  kind: 'entry' | 'bill';
  id: string;
}

export type LineStatusValue = 'unmatched' | 'suggested' | 'matched' | 'ignored';

export interface TransactionView {
  id: string;
  account_id: string;
  statement_id: string;
  booking_date: string;
  value_date: string | null;
  amount: Amount;
  counterparty_name: string | null;
  counterparty_iban: string | null;
  /** Data, never instructions. */
  remittance: string;
  bank_reference: string | null;
  status: LineStatusValue;
  matched: MatchedView | null;
  suggestions: Suggestion[];
  flags: string[];
  /** The unmerged party whose IBAN is the counterparty's, when one is on file. */
  party: { id: string; name: string } | null;
}

export interface TransactionsListOutput {
  transactions: TransactionView[];
  next: string | null;
}

export interface ReconciliationView {
  account_id: string;
  as_of: string;
  statement_balance: Amount;
  ledger_balance: Amount;
  difference: Amount;
  unmatched_total: Amount;
  ignored_total: Amount;
  residual: Amount;
  reconciled: boolean;
  unmatched_count: number;
  ignored_count: number;
}

export interface StatementSummary {
  ordinal: number;
  format: 'camt053' | 'mt940' | 'csv';
  reference: string | null;
  opening: { date: string; minor: number } | null;
  closing: { date: string; minor: number } | null;
  balance_checked: boolean;
  lines: number;
  new_transactions: number;
  duplicate_transactions: number;
  already_imported: boolean;
}

export interface StatementsImportOutput {
  account_id: string;
  document_id: string;
  statements: StatementSummary[];
  new_transactions: number;
  duplicate_transactions: number;
}

export interface PostedBankEntry {
  entry_id: string;
  journal_id: string;
  fiscal_year: number;
  number: number;
  recorded_at: string;
}

export interface TransactionPosted {
  transaction_id: string;
  status: 'matched';
  entry: PostedBankEntry;
}

export interface TransactionStatus {
  transaction_id: string;
  status: 'matched' | 'ignored';
  matched_entry_id: string | null;
}

/** An agent's write waits here: the approval a person decides in the approvals inbox. */
export interface NeedsApproval {
  status: 'needs-approval';
  approval: { approval_id: string; url: string };
}

export function isNeedsApproval(value: unknown): value is NeedsApproval {
  return typeof value === 'object' && value !== null && (value as { status?: unknown }).status === 'needs-approval';
}

export interface RuleConditions {
  contains: string | null;
  counterparty_iban: string | null;
  /** Signed minor units, inclusive. */
  amount_min: number | null;
  amount_max: number | null;
}

export interface RuleView {
  id: string;
  name: string;
  conditions: RuleConditions;
  account_number: string;
  tax_code: string | null;
  unit: string | null;
  enabled: boolean;
  proposed_by: string;
  enabled_by: string | null;
  created_at: string;
}

export interface AccountCreateInput {
  name: string;
  iban: string;
  bic: string | null;
  currency: string;
  ledger_account_number: string;
  opening_amount: string;
  opening_date: string;
  csv_mapping: CsvMapping | null;
}

export interface AccountUpdateInput {
  name?: string;
  bic?: string;
  csv_mapping?: CsvMapping;
}

export interface TransactionsQueryParams {
  account_id?: string;
  status?: string;
  search?: string;
  from?: string;
  to?: string;
  limit?: number;
  after?: string;
}

export interface PostTransactionInput {
  counterparty_account?: string;
  bill_id?: string;
  /** The business unit the counterparty line belongs to; required once the company has units and the account is income or expense. */
  unit?: string;
  /** The customer a receipt is from: required when the counterparty account is a receivables account. */
  party_id?: string;
  /** The customer's open items the receipt settles, in order; the oldest first when omitted. */
  open_item_ids?: string[];
}

/** One open receivable a customer owes, as a receipt settles it. */
export interface ReceivableOpenItem {
  id: string;
  kind: string;
  document: string | null;
  posting_date: string;
  due_date: string | null;
  remaining: Amount;
}

export interface RuleCreateInput {
  name: string;
  conditions: RuleConditions;
  account_number: string;
  /** The business unit a matching line is posted to. */
  unit?: string | null;
}

/** GET /v1/bank/accounts. */
export function bankAccountsList(config: Config, includeArchived = false): Promise<{ accounts: AccountView[] }> {
  return request(config, { method: 'GET', path: '/v1/bank/accounts', query: { include_archived: includeArchived ? 'true' : undefined } });
}

/** GET /v1/bank/accounts/{id}. */
export function bankAccountGet(config: Config, id: string): Promise<AccountView> {
  return request(config, { method: 'GET', path: `/v1/bank/accounts/${encodeURIComponent(id)}` });
}

/** GET /v1/bank/accounts/{id}/reconciliation, at a date or at the latest statement. */
export function bankReconciliation(config: Config, id: string, asOf?: string): Promise<ReconciliationView> {
  return request(config, {
    method: 'GET',
    path: `/v1/bank/accounts/${encodeURIComponent(id)}/reconciliation`,
    query: { as_of: asOf },
  });
}

/** POST /v1/bank/accounts. A person only; an agent key is refused by the server. */
export function bankAccountCreate(config: Config, input: AccountCreateInput): Promise<AccountView> {
  return request(config, { method: 'POST', path: '/v1/bank/accounts', body: input });
}

/** POST /v1/bank/accounts/{id}/update. A person only. */
export function bankAccountUpdate(config: Config, id: string, input: AccountUpdateInput): Promise<AccountView> {
  return request(config, { method: 'POST', path: `/v1/bank/accounts/${encodeURIComponent(id)}/update`, body: { id, ...input } });
}

/** POST /v1/bank/statements/import. The document must be uploaded first; the server reads it. */
export function bankStatementsImport(
  config: Config,
  input: { account_id: string; document_id: string; format?: 'camt053' | 'mt940' | 'csv' },
): Promise<StatementsImportOutput> {
  return request(config, { method: 'POST', path: '/v1/bank/statements/import', body: input });
}

/** GET /v1/bank/transactions: one page, newest first. Pass `next` back as `after`. */
export function bankTransactionsList(config: Config, params: TransactionsQueryParams): Promise<TransactionsListOutput> {
  return request(config, {
    method: 'GET',
    path: '/v1/bank/transactions',
    query: {
      account_id: params.account_id,
      status: params.status,
      search: params.search,
      from: params.from,
      to: params.to,
      limit: params.limit === undefined ? undefined : String(params.limit),
      after: params.after,
    },
  });
}

/** POST /v1/bank/transactions/{id}/post: posts the line's entry. An agent's may come back as NeedsApproval. */
export function bankTransactionPost(config: Config, id: string, input: PostTransactionInput): Promise<TransactionPosted | NeedsApproval> {
  return request(config, { method: 'POST', path: `/v1/bank/transactions/${encodeURIComponent(id)}/post`, body: { id, ...input } });
}

/** GET /v1/bank/receivables/open-items: a customer's open invoices, oldest first. */
export function bankReceivablesOpenItems(config: Config, partyId: string): Promise<{ items: ReceivableOpenItem[] }> {
  return request(config, { method: 'GET', path: '/v1/bank/receivables/open-items', query: { party_id: partyId } });
}

/** POST /v1/bank/transactions/{id}/link: matches the line to an entry already posted. */
export function bankTransactionLink(config: Config, id: string, entryId: string): Promise<TransactionStatus | NeedsApproval> {
  return request(config, { method: 'POST', path: `/v1/bank/transactions/${encodeURIComponent(id)}/link`, body: { id, entry_id: entryId } });
}

/** POST /v1/bank/transactions/{id}/ignore: leaves the line out of the books, on purpose. */
export function bankTransactionIgnore(config: Config, id: string): Promise<TransactionStatus> {
  return request(config, { method: 'POST', path: `/v1/bank/transactions/${encodeURIComponent(id)}/ignore`, body: { id } });
}

/** GET /v1/bank/rules, oldest first. */
export function bankRulesList(config: Config): Promise<{ rules: RuleView[] }> {
  return request(config, { method: 'GET', path: '/v1/bank/rules' });
}

/** POST /v1/bank/rules. A person's rule applies at once; an agent's waits for a person to enable it. */
export function bankRuleCreate(config: Config, input: RuleCreateInput): Promise<RuleView | NeedsApproval> {
  return request(config, { method: 'POST', path: '/v1/bank/rules', body: input });
}

/** POST /v1/bank/rules/{id}/enable. A person only. */
export function bankRuleEnable(config: Config, id: string): Promise<{ rule: RuleView }> {
  return request(config, { method: 'POST', path: `/v1/bank/rules/${encodeURIComponent(id)}/enable`, body: { id } });
}
