/**
 * The Settings commands (crates/ledgers-server/src/dispatch/settings.rs). One
 * function per route; every route is person-only, so an agent key gets 403.
 */
import { request, type ClientConfig } from './client';

export type Config = ClientConfig;

export interface CompanyProfile {
  legal_form: string;
  vat_id: string;
  registration_number: string;
  address_line: string;
  postal_code: string;
  city: string;
  country: string;
  contact_email: string;
}

export interface CompanyView {
  name: string;
  base_currency: string;
  base_currency_locked: boolean;
  profile: CompanyProfile;
}

export type PeriodState = 'open' | 'soft_closed' | 'closed';

export interface PeriodView {
  period: string;
  state: PeriodState;
}

export interface FiscalYearView {
  label: number;
  starts_on: string;
  ends_on: string;
  closed: boolean;
  closed_at: string | null;
  periods: PeriodView[];
}

export interface FiscalYearCloseOutput {
  year: FiscalYearView;
  closing_entry_id: string | null;
}

export interface SequenceView {
  fiscal_year: number;
  next_number: number;
  entries: number;
  editable: boolean;
}

export interface JournalNumberingView {
  id: string;
  code: string;
  name: string;
  kind: string;
  sequences: SequenceView[];
}

export interface TaxRateView {
  category: 'standard' | 'reduced';
  percent: string;
  source: string;
}

export interface TaxSettingsView {
  small_business: boolean;
  vat_basis: 'accrual' | 'cash';
  country: string;
  rate_table: boolean;
  rates: TaxRateView[];
}

export interface ApprovalSettingsView {
  low_risk_limits: Record<string, string>;
  request_ttl_hours: number;
  fresh_sign_in_minutes: number;
}

export interface AuditEventView {
  id: string;
  seq: number;
  at: string;
  actor_kind: 'person' | 'agent';
  actor_id: string;
  command: string;
  outcome: 'allowed' | 'needs-approval' | 'denied' | 'error';
  objects: string[];
  reason: string | null;
  request_id: string;
}

export interface AuditListParams {
  before_seq?: number;
  limit?: number;
  actor_kind?: string;
  command?: string;
  outcome?: string;
  from?: string;
  to?: string;
}

export interface AuditListOutput {
  events: AuditEventView[];
  next_before_seq: number | null;
}

/** GET /v1/company (company.get). */
export function companyGet(config: Config): Promise<CompanyView> {
  return request<CompanyView>(config, { method: 'GET', path: '/v1/company' });
}

/** POST /v1/company (company.update): the name and the profile, audited with the change. */
export function companyUpdate(config: Config, input: { name: string; profile: CompanyProfile }): Promise<CompanyView> {
  return request<CompanyView>(config, { method: 'POST', path: '/v1/company', body: input });
}

/** GET /v1/fiscal-years (fiscal_years.list). */
export function fiscalYearsList(config: Config): Promise<{ years: FiscalYearView[] }> {
  return request<{ years: FiscalYearView[] }>(config, { method: 'GET', path: '/v1/fiscal-years' });
}

/** POST /v1/fiscal-years (fiscal_years.create): opens a January-to-December year. */
export function fiscalYearCreate(config: Config, label: number): Promise<FiscalYearView> {
  return request<FiscalYearView>(config, { method: 'POST', path: '/v1/fiscal-years', body: { label } });
}

/** POST /v1/fiscal-years/close (fiscal_years.close): locks the year's months and runs the year-end close. */
export function fiscalYearClose(config: Config, label: number): Promise<FiscalYearCloseOutput> {
  return request<FiscalYearCloseOutput>(config, { method: 'POST', path: '/v1/fiscal-years/close', body: { label } });
}

/** POST /v1/periods/close (periods.close): locks one month against postings. */
export function periodClose(config: Config, period: string): Promise<PeriodView> {
  return request<PeriodView>(config, { method: 'POST', path: '/v1/periods/close', body: { period } });
}

/** POST /v1/periods/reopen (periods.reopen): reopens a month; the reason is kept in the audit trail. */
export function periodReopen(config: Config, period: string, reason: string): Promise<PeriodView> {
  return request<PeriodView>(config, { method: 'POST', path: '/v1/periods/reopen', body: { period, reason } });
}

/** GET /v1/numbering (numbering.list). */
export function numberingList(config: Config): Promise<{ journals: JournalNumberingView[] }> {
  return request<{ journals: JournalNumberingView[] }>(config, { method: 'GET', path: '/v1/numbering' });
}

/** POST /v1/numbering/next (numbering.set_next): moves a sequence's next number forward while nothing is issued. */
export function numberingSetNext(
  config: Config,
  input: { journal_id: string; fiscal_year: number; next_number: number },
): Promise<{ journal_id: string; fiscal_year: number; next_number: number }> {
  return request(config, { method: 'POST', path: '/v1/numbering/next', body: input });
}

/** GET /v1/tax-settings (tax_settings.get). */
export function taxSettingsGet(config: Config): Promise<TaxSettingsView> {
  return request<TaxSettingsView>(config, { method: 'GET', path: '/v1/tax-settings' });
}

/** POST /v1/tax-settings (tax_settings.update). */
export function taxSettingsUpdate(
  config: Config,
  input: { small_business: boolean; vat_basis: 'accrual' | 'cash' },
): Promise<TaxSettingsView> {
  return request<TaxSettingsView>(config, { method: 'POST', path: '/v1/tax-settings', body: input });
}

/** GET /v1/approval-settings (approval_settings.get). */
export function approvalSettingsGet(config: Config): Promise<ApprovalSettingsView> {
  return request<ApprovalSettingsView>(config, { method: 'GET', path: '/v1/approval-settings' });
}

/** POST /v1/approval-settings (approval_settings.update). */
export function approvalSettingsUpdate(
  config: Config,
  input: { low_risk_limits: Record<string, string>; request_ttl_hours: number },
): Promise<ApprovalSettingsView> {
  return request<ApprovalSettingsView>(config, { method: 'POST', path: '/v1/approval-settings', body: input });
}

/** GET /v1/audit-events (audit.list): one page, newest first. */
export function auditList(config: Config, params: AuditListParams = {}): Promise<AuditListOutput> {
  return request<AuditListOutput>(config, {
    method: 'GET',
    path: '/v1/audit-events',
    query: {
      before_seq: params.before_seq === undefined ? undefined : String(params.before_seq),
      limit: params.limit === undefined ? undefined : String(params.limit),
      actor_kind: params.actor_kind || undefined,
      command: params.command || undefined,
      outcome: params.outcome || undefined,
      from: params.from || undefined,
      to: params.to || undefined,
    },
  });
}

/** The extraction reader and the features that are off (GET /v1/status, no credential needed). */
export interface StatusView {
  ok: boolean;
  extraction: { provider: string; configured: string | null; reason: 'missing_key' | 'unknown_provider' | null };
  unavailable: { feature: string; reason: string }[];
  limits: { document_max_bytes: number; pdf_max_pages: number };
}

export function statusGet(config: Config): Promise<StatusView> {
  return request<StatusView>(config, { method: 'GET', path: '/v1/status' });
}
