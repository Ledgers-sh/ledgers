/**
 * Customers and vendors (issue #44): one function per party command. The app
 * holds no token; every call sends the session cookie. Bank-detail changes and
 * merges are person-only on the server (an agent key gets 403).
 */
import { request, type ClientConfig } from './client';

export type PartyRoles = 'customer' | 'vendor' | 'both';

export interface PartyAddress {
  kind: string | null;
  line1: string;
  line2: string | null;
  postal_code: string | null;
  city: string;
  /** ISO 3166-1 alpha-2. */
  country: string;
}

export interface PartyContact {
  name: string;
  email: string | null;
  phone: string | null;
  role: string | null;
  /** The party's primary contact; at most one per party. */
  primary: boolean;
}

/** A further VAT or tax identifier: a VAT ID of another country, or a tax number. */
export interface PartyTaxId {
  /** ISO 3166-1 alpha-2. */
  country: string;
  kind: 'vat' | 'tax' | 'other';
  /** The identifier; a VAT ID carries its country prefix. */
  value: string;
}

export interface PartyViesResult {
  valid: boolean;
  consultation_number: string | null;
  checked_at: string;
}

export interface Party {
  id: string;
  name: string;
  vat_id: string | null;
  iban: string | null;
  bic: string | null;
  roles: PartyRoles;
  merged_into: string | null;
  latest_vies: PartyViesResult | null;
  personal_data: boolean;
  possible_duplicates: string[];
  needs_review: boolean;
  tax_number: string | null;
  language: string | null;
  currency: string | null;
  payment_terms_days: number | null;
  default_account: string | null;
  default_tax_code: string | null;
  addresses: PartyAddress[];
  contacts: PartyContact[];
  trading_name: string | null;
  registration_number: string | null;
  website: string | null;
  email: string | null;
  phone: string | null;
  default_unit: string | null;
  tags: string[];
  notes: string | null;
  tax_ids: PartyTaxId[];
  /** The party's logo document (PNG or JPEG), when it has one. */
  logo_document_id: string | null;
}

export interface PartyBankChange {
  id: string;
  before_iban: string | null;
  before_bic: string | null;
  after_iban: string | null;
  after_bic: string | null;
  changed_by: string;
  changed_at: string;
  reason: string | null;
}

/** A bill of the party, as the detail page lists it. */
export interface PartyRecentBill {
  id: string;
  invoice_number: string | null;
  invoice_date: string | null;
  due_date: string | null;
  currency: string;
  gross_minor: number | null;
  paid_minor: number;
  posted: boolean;
  discarded: boolean;
}

/** A payment recorded against one of the party's bills. */
export interface PartyPayment {
  bill_id: string;
  invoice_number: string | null;
  currency: string;
  paid_minor: number;
  paid_at: string;
}

export interface PartyDetail {
  party: Party;
  bank_changes: PartyBankChange[];
  bill_count: number;
  vies_enabled: boolean;
  recent_bills: PartyRecentBill[];
  recent_payments: PartyPayment[];
}

/** One receivable (`ar`) or payable (`ap`) still partly or wholly open. */
export interface PartyOpenItem {
  id: string;
  ledger: 'ar' | 'ap';
  kind: 'invoice' | 'credit-note' | 'bill' | 'on-account';
  document: string | null;
  posting_date: string;
  due_date: string | null;
  currency: string;
  original_minor: number;
  remaining_minor: number;
}

/** What is still open for a party, per ledger and currency. */
export interface PartyOpenBalance {
  ledger: 'ar' | 'ap';
  currency: string;
  remaining_minor: number;
}

export interface PartyOpenItems {
  open_items: PartyOpenItem[];
  balances: PartyOpenBalance[];
}

/** One row of a CSV check: its line in the file, what it would do, and its refusals. */
export interface PartyImportRow {
  line: number;
  action: 'create' | 'update' | 'error';
  id: string;
  errors: { slug: string; field: string; message: string }[];
}

export interface PartyImportCheck {
  valid: boolean;
  creates: number;
  updates: number;
  errors: number;
  rows: PartyImportRow[];
}

export interface PartyImportResult {
  created: number;
  updated: number;
  ids: string[];
}

export interface PartiesPage {
  parties: Party[];
  total: number;
}

export interface PartyBalance {
  party_id: string;
  currency: string;
  payable_minor: number;
  open_bills: number;
}

export interface PartyFieldError {
  field: string;
  message: string;
  /** The problem slug of the refusal, when the server names one; the app words it in its language. */
  slug?: string | null;
}

export interface PartyValidation {
  valid: boolean;
  errors: PartyFieldError[];
}

/** The filters of the list screen; blank filters are left out of the request. */
export interface PartyListQuery {
  limit?: number;
  offset?: number;
  q?: string;
  roles?: PartyRoles;
  needs_review?: boolean;
  tag?: string;
}

/** The fields `parties.create` takes. Blank text is sent as an empty string and stored as none. */
export interface PartyCreateInput {
  id?: string;
  name: string;
  trading_name?: string | null;
  vat_id?: string | null;
  tax_ids?: PartyTaxId[];
  roles?: PartyRoles;
  tax_number?: string | null;
  registration_number?: string | null;
  website?: string | null;
  email?: string | null;
  phone?: string | null;
  language?: string | null;
  currency?: string | null;
  personal_data?: boolean;
  payment_terms_days?: number | null;
  default_account?: string | null;
  default_tax_code?: string | null;
  default_unit?: string | null;
  tags?: string[];
  notes?: string | null;
  addresses?: PartyAddress[];
  contacts?: PartyContact[];
}

/** The fields `parties.update` changes; a field left out keeps its value. */
export type PartyUpdateInput = Partial<Omit<PartyCreateInput, 'id'>>;

/** The fields `parties.validate` checks; a field left out is not checked. */
export interface PartyValidateInput {
  id?: string;
  name?: string;
  vat_id?: string;
  iban?: string;
  bic?: string;
  roles?: PartyRoles;
  payment_terms_days?: number;
  addresses?: PartyAddress[];
  contacts?: PartyContact[];
  registration_number?: string;
  website?: string;
  email?: string;
  phone?: string;
  currency?: string;
  language?: string;
  default_unit?: string;
  notes?: string;
  tags?: string[];
  tax_ids?: PartyTaxId[];
}

function query(params: PartyListQuery): Record<string, string | undefined> {
  return {
    limit: params.limit === undefined ? undefined : String(params.limit),
    offset: params.offset === undefined ? undefined : String(params.offset),
    q: params.q && params.q.trim() !== '' ? params.q.trim() : undefined,
    roles: params.roles,
    needs_review: params.needs_review === undefined ? undefined : String(params.needs_review),
    tag: params.tag && params.tag.trim() !== '' ? params.tag.trim().toLowerCase() : undefined,
  };
}

/** GET /v1/parties: one page of unmerged parties, with the total of every match. */
export function partiesList(config: ClientConfig, params: PartyListQuery = {}): Promise<PartiesPage> {
  return request<PartiesPage>(config, { method: 'GET', path: '/v1/parties', query: query(params) });
}

/** GET /v1/parties/{id}: one party with its bank history and bill count. */
export function partyGet(config: ClientConfig, id: string): Promise<PartyDetail> {
  return request<PartyDetail>(config, { method: 'GET', path: `/v1/parties/${encodeURIComponent(id)}` });
}

/** GET /v1/parties/balances: open payables per party and currency (receivables are not tracked yet). */
export function partiesBalances(config: ClientConfig): Promise<{ balances: PartyBalance[] }> {
  return request<{ balances: PartyBalance[] }>(config, { method: 'GET', path: '/v1/parties/balances' });
}

/** POST /v1/parties/validate: the server's own checks, with no write. */
export function partyValidate(config: ClientConfig, input: PartyValidateInput): Promise<PartyValidation> {
  return request<PartyValidation>(config, { method: 'POST', path: '/v1/parties/validate', body: input });
}

/** POST /v1/parties: creates a party. */
export function partyCreate(config: ClientConfig, input: PartyCreateInput): Promise<{ party: Party }> {
  return request<{ party: Party }>(config, { method: 'POST', path: '/v1/parties', body: input });
}

/** POST /v1/parties/{id}/update: changes master data (not bank details). */
export function partyUpdate(config: ClientConfig, id: string, input: PartyUpdateInput): Promise<{ party: Party }> {
  return request<{ party: Party }>(config, {
    method: 'POST',
    path: `/v1/parties/${encodeURIComponent(id)}/update`,
    body: input,
  });
}

/** POST /v1/parties/{id}/check-vat: a VIES lookup of the party's VAT ID (only when the server has it on). */
export function partyCheckVat(
  config: ClientConfig,
  id: string,
  country: string,
  vatNumber: string,
): Promise<{ vies_result: { valid: boolean; name: string | null; address: string | null; consultation_number: string | null; request_date: string | null } }> {
  return request(config, {
    method: 'POST',
    path: `/v1/parties/${encodeURIComponent(id)}/check-vat`,
    body: { country_code: country, vat_number: vatNumber },
  });
}

/** POST /v1/parties/{id}/set-bank: a person changes the IBAN and BIC; the change is recorded with its reason. */
export function partySetBank(
  config: ClientConfig,
  id: string,
  input: { iban: string; bic: string | null; reason: string },
): Promise<{ party: Party }> {
  return request<{ party: Party }>(config, {
    method: 'POST',
    path: `/v1/parties/${encodeURIComponent(id)}/set-bank`,
    body: input,
  });
}

/** POST /v1/parties/{id}/merge: folds `from` into `id` (the survivor); `from`'s bills move to it. */
export function partyMerge(config: ClientConfig, survivorId: string, fromId: string): Promise<{ party: Party }> {
  return request<{ party: Party }>(config, {
    method: 'POST',
    path: `/v1/parties/${encodeURIComponent(survivorId)}/merge`,
    body: { from: fromId },
  });
}

/** GET /v1/parties/{id}/open-items: what the party still owes or is owed, from the subledger. */
export function partyOpenItems(config: ClientConfig, id: string): Promise<PartyOpenItems> {
  return request<PartyOpenItems>(config, { method: 'GET', path: `/v1/parties/${encodeURIComponent(id)}/open-items` });
}

/** POST /v1/parties/{id}/set-logo: sets the logo to a stored PNG or JPEG document, or clears it with null. */
export function partySetLogo(config: ClientConfig, id: string, documentId: string | null): Promise<{ party: Party }> {
  return request<{ party: Party }>(config, {
    method: 'POST',
    path: `/v1/parties/${encodeURIComponent(id)}/set-logo`,
    body: { logo_document_id: documentId },
  });
}

/** GET /v1/parties/export: every party as CSV, in the columns the import reads. */
export function partyExport(config: ClientConfig): Promise<{ csv: string; rows: number }> {
  return request<{ csv: string; rows: number }>(config, { method: 'GET', path: '/v1/parties/export' });
}

/** POST /v1/parties/import/check: reads a CSV file and reports each row, writing nothing. */
export function partyImportCheck(config: ClientConfig, csv: string): Promise<PartyImportCheck> {
  return request<PartyImportCheck>(config, { method: 'POST', path: '/v1/parties/import/check', body: { csv } });
}

/** POST /v1/parties/import: writes a CSV file whose rows are all valid; a file with any error is refused whole. */
export function partyImport(config: ClientConfig, csv: string): Promise<PartyImportResult> {
  return request<PartyImportResult>(config, { method: 'POST', path: '/v1/parties/import', body: { csv } });
}
