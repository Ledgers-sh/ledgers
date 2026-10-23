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

export interface PartyDetail {
  party: Party;
  bank_changes: PartyBankChange[];
  bill_count: number;
  vies_enabled: boolean;
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
}

/** The fields `parties.create` takes. Blank text is sent as an empty string and stored as none. */
export interface PartyCreateInput {
  id?: string;
  name: string;
  vat_id?: string | null;
  roles?: PartyRoles;
  tax_number?: string | null;
  language?: string | null;
  currency?: string | null;
  personal_data?: boolean;
  payment_terms_days?: number | null;
  default_account?: string | null;
  default_tax_code?: string | null;
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
}

function query(params: PartyListQuery): Record<string, string | undefined> {
  return {
    limit: params.limit === undefined ? undefined : String(params.limit),
    offset: params.offset === undefined ? undefined : String(params.offset),
    q: params.q && params.q.trim() !== '' ? params.q.trim() : undefined,
    roles: params.roles,
    needs_review: params.needs_review === undefined ? undefined : String(params.needs_review),
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
