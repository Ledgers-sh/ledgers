/**
 * Documents and vendor bills (issues #20 to #22). The HTTP shapes come from
 * crates/ledgers-server (documents, bills) and crates/ledgers-bills. Upload is
 * the one call with a raw body; every other call is a registry command.
 */
import { ApiError, buildUrl, kindForStatus, request, requestInit, type ClientConfig } from './client';
import type { Account } from './types';

export type Config = ClientConfig;

/** Lifecycle of a bill, derived by the server on every read. */
export type BillStatus = 'needs-review' | 'ready' | 'posted' | 'discarded';

export interface Document {
  id: string;
  sha256: string;
  size_bytes: number;
  mime: string;
  original_name: string;
  uploaded_by: string;
  retention_class: 'ten-years' | 'six-years' | 'indefinite';
  created_at: string;
  has_text_layer: boolean;
}

export interface BillLine {
  description: string;
  net_minor: number | null;
  vat_rate_bp: number | null;
  vat_minor: number | null;
  gross_minor: number | null;
  expense_account: string | null;
  suggested_from: string | null;
}

export interface BillCheck {
  id: string;
  ok: boolean;
  detail: string;
}

export interface BillVendor {
  id: string;
  name: string;
  vat_id: string | null;
  iban: string | null;
  needs_review: boolean;
  /** The party's logo document, when it has one. */
  logo_document_id?: string | null;
}

export interface PreviewLine {
  account: string;
  side: 'debit' | 'credit';
  minor: number;
}

export interface Bill {
  id: string;
  status: BillStatus;
  document_id: string;
  vendor: BillVendor;
  invoice_number: string | null;
  invoice_date: string | null;
  due_date: string | null;
  currency: string;
  net_minor: number | null;
  vat_minor: number | null;
  gross_minor: number | null;
  lines: BillLine[];
  checks: BillCheck[];
  flags: string[];
  /** Confidence per extracted field, 0 to 100. Empty for manual bills. */
  confidence: Record<string, number>;
  source: string;
  /** Which reader produced the fields: rules, anthropic, minimax, unread-scan; null when typed by hand. */
  reader: string | null;
  posted_entry_id: string | null;
  /** The business unit the bill's expense lines belong to; null when none is set. */
  unit: string | null;
  journal_preview: PreviewLine[];
  created_by: string;
  created_at: string;
  updated_at: string;
}

export interface BillExtraction {
  bill: Bill;
  document: Document;
  extraction: { mode: 'model' | 'text-layer' | 'manual'; warnings: string[]; needs_photo: boolean };
}

/** The fields a person can correct on a bill. Amounts are minor units. */
export interface BillFields {
  /** The party the bill is for; a picked party re-points the bill. */
  vendor_id?: string | null;
  vendor_name?: string | null;
  vendor_vat_id?: string | null;
  vendor_iban?: string | null;
  invoice_number: string | null;
  invoice_date: string | null;
  due_date: string | null;
  currency: string;
  net_minor: number | null;
  vat_minor: number | null;
  gross_minor: number | null;
  lines: BillLine[];
  /** The business unit the expense lines belong to; required to post once the company has units. */
  unit?: string | null;
}

export interface AccountChoice {
  number: string;
  /** The chart's name key; the UI translates it with the chart names. */
  name: string;
  type: string;
}

/** A problem from the server, with the problem body kept for its extension members. */
export class BillApiError extends ApiError {
  readonly body: Record<string, unknown>;
  constructor(kind: ApiError['kind'], message: string, status: number, detail: string | null, slug: string | null, body: Record<string, unknown>) {
    super(kind, message, status, detail, slug);
    this.body = body;
  }

  /** The bill a duplicate refusal names (`existing_bill_id`), or null. */
  get existingBillId(): string | null {
    const id = this.body.existing_bill_id;
    return typeof id === 'string' && id !== '' ? id : null;
  }
}

/** Like `request`, but a failure keeps the problem body (the extension members). */
async function billRequest<T>(config: Config, method: 'GET' | 'POST', path: string, body?: unknown): Promise<T> {
  const doFetch = config.fetch ?? fetch;
  const headers: Record<string, string> = { Accept: 'application/json' };
  const init = requestInit(config, method, headers);
  if (body !== undefined) {
    headers['Content-Type'] = 'application/json';
    init.body = JSON.stringify(body);
  }
  let response: Response;
  try {
    response = await doFetch(buildUrl(config.baseUrl, path), init);
  } catch {
    throw new ApiError('network', 'The server could not be reached.', null);
  }
  const parsed = (await response.json().catch(() => ({}))) as Record<string, unknown>;
  if (!response.ok) {
    const kind = kindForStatus(response.status);
    const detail = typeof parsed.detail === 'string' && parsed.detail !== '' ? parsed.detail : null;
    const slug = typeof parsed.type === 'string' ? parsed.type.split('/').pop() ?? null : null;
    const message = kind === 'validation' && detail ? detail : kindMessage(kind);
    throw new BillApiError(kind, message, response.status, detail, slug, parsed);
  }
  return parsed as T;
}

function kindMessage(kind: ApiError['kind']): string {
  switch (kind) {
    case 'unauthorized':
      return 'Sign in or use a key that may do this.';
    case 'forbidden':
      return 'This key is not allowed to do this.';
    case 'not_found':
      return 'Not found.';
    case 'network':
      return 'The server could not be reached.';
    default:
      return 'Something went wrong on the server.';
  }
}

/** The upload response: the document, and whether these bytes were already stored. */
export interface UploadResult {
  document: Document;
  duplicate: boolean;
  warning: string | null;
}

/**
 * POST /v1/documents/upload. The body is the file itself; the name travels in
 * `X-File-Name`. The server sniffs the type from the bytes.
 */
export async function uploadDocument(config: Config, file: Blob, name: string): Promise<UploadResult> {
  const doFetch = config.fetch ?? fetch;
  const headers: Record<string, string> = { Accept: 'application/json', 'X-File-Name': encodeURIComponent(name) };
  if (file.type) headers['Content-Type'] = file.type;
  const init = requestInit(config, 'POST', headers);
  init.body = file;
  let response: Response;
  try {
    response = await doFetch(buildUrl(config.baseUrl, '/v1/documents/upload'), init);
  } catch {
    throw new ApiError('network', 'The server could not be reached.', null);
  }
  const body = (await response.json().catch(() => ({}))) as Record<string, unknown>;
  if (!response.ok) {
    throw new ApiError(kindForStatus(response.status), uploadMessage(response.status), response.status, String(body.detail ?? ''), String(body.type ?? ''));
  }
  return body as unknown as UploadResult;
}

function uploadMessage(status: number): string {
  if (status === 413) return 'This file is larger than 10 MB.';
  if (status === 401 || status === 403) return 'This key may not upload documents.';
  return 'The file could not be stored.';
}

/** POST /v1/bills/extract: reads the stored document into a draft bill. */
export function extractBill(config: Config, documentId: string): Promise<BillExtraction> {
  return billRequest<BillExtraction>(config, 'POST', '/v1/bills/extract', { document_id: documentId });
}

/** GET /v1/bills, newest first; `unit` narrows the list to one business unit (a code), or every unit when absent. */
export function billsList(config: Config, unit?: string | null): Promise<{ bills: Bill[] }> {
  const scope = unit ? `&unit=${encodeURIComponent(unit)}` : '';
  return billRequest(config, 'GET', `/v1/bills?limit=100${scope}`);
}

export function billGet(config: Config, id: string): Promise<{ bill: Bill }> {
  return billRequest(config, 'GET', `/v1/bills/${encodeURIComponent(id)}`);
}

export function billUpdate(config: Config, id: string, fields: BillFields): Promise<{ bill: Bill }> {
  return billRequest(config, 'POST', `/v1/bills/${encodeURIComponent(id)}/update`, fields);
}

export function billConfirm(config: Config, id: string): Promise<{ bill: Bill }> {
  return billRequest(config, 'POST', `/v1/bills/${encodeURIComponent(id)}/confirm`, {});
}

export function billDiscard(config: Config, id: string): Promise<{ bill: Bill }> {
  return billRequest(config, 'POST', `/v1/bills/${encodeURIComponent(id)}/discard`, {});
}

export interface PostedBill {
  bill: Bill;
  entry: { entry_id: string; journal_id: string; fiscal_year: number; number: number; recorded_at: string };
}

/** POST /v1/bills/{id}/post. A person posts directly; an agent's post may come back as needs-approval. */
export function billPost(config: Config, id: string): Promise<PostedBill> {
  return billRequest(config, 'POST', `/v1/bills/${encodeURIComponent(id)}/post`, {});
}

/** GET /v1/documents/{id}: the stored document's metadata. */
export function documentGet(config: Config, id: string): Promise<{ document: Document }> {
  return billRequest(config, 'GET', `/v1/documents/${encodeURIComponent(id)}`);
}

/** GET /v1/documents/{id}/url: a link that serves the file for five minutes. */
export function documentUrl(config: Config, id: string): Promise<{ url: string; expires_at: number }> {
  return billRequest(config, 'GET', `/v1/documents/${encodeURIComponent(id)}/url`);
}

/** The absolute address of a signed document link, on the API origin. */
export function absoluteUrl(config: Config, relative: string): string {
  return `${config.baseUrl.replace(/\/$/, '')}${relative}`;
}

/** The chart, for the expense account choices. */
export async function expenseAccounts(config: Config): Promise<AccountChoice[]> {
  const response = await request<{ accounts: Account[] }>(config, { method: 'GET', path: '/v1/accounts' });
  return response.accounts
    .filter((account) => account.account_type === 'expense' && account.is_postable)
    .map((account) => ({ number: account.number, name: account.name_key, type: account.account_type }));
}
