/**
 * Groups, intercompany agreements and the group reports (ledgers-group, the
 * contract in PR #141). Every call is a person's: an agent key gets 403
 * "human-only". A call acts for the company the page is in; agreements are
 * written in both companies, so the server checks both.
 */
import { request, type ClientConfig } from './client';

export type GroupKind = 'holding' | 'ip_holding' | 'operating';
export type AgreementKind = 'licence' | 'royalty' | 'management_fee' | 'loan';
export type AgreementBasis = 'pct_of_revenue_bp' | 'fixed_monthly' | 'cost_plus_bp';
/** How the withholding rate was found: the payer's country pack, a treaty override, or why there is none. */
export type WhtBasis = 'pack' | 'treaty_override' | 'resident' | 'no_rate_on_file' | 'not_applicable';
export type Side = 'licensor' | 'licensee';
/** A period document: `failed` is a post that was reversed; a retry starts again from it. */
export type DocStatus = 'draft' | 'posted' | 'settled' | 'failed';

export interface CompanyRef {
  id: string;
  name: string;
}

export interface MemberView {
  company_id: string;
  company_name: string;
  kind: GroupKind;
  ownership_bp: number;
}

export interface GroupView {
  id: string;
  name: string;
  /** The group's logo document, and the company whose documents hold it. */
  logo_document_id?: string | null;
  logo_company_id?: string | null;
  members: MemberView[];
  has_hidden_members: boolean;
  created_by: string;
  created_at: string;
  updated_at: string;
}

export interface MemberInput {
  company_id: string;
  kind: GroupKind;
  ownership_bp: number;
}

export interface AccountsInput {
  royalty_account: string;
  ic_account: string;
  wht_account?: string | null;
  bank_account: string;
  unit?: string | null;
}

export interface AccountsView {
  royalty: string;
  ic: string;
  wht: string | null;
  bank: string;
  unit: string | null;
}

export interface AgreementView {
  id: string;
  group_id: string;
  side: Side;
  counterparty_id: string | null;
  counterparty_hidden: boolean;
  kind: AgreementKind;
  basis: AgreementBasis;
  rate_bp: number | null;
  fixed_minor: number | null;
  currency: string;
  wht_bp: number;
  wht_basis: WhtBasis;
  treaty_override_bp: number | null;
  accounts: AccountsView;
  notes: string | null;
  terminated_at: string | null;
  created_by: string;
  created_at: string;
  updated_at: string;
}

export interface PeriodDocView {
  id: string;
  side: Side;
  period: string;
  status: DocStatus;
  currency: string;
  basis_minor: number;
  gross_minor: number;
  wht_minor: number;
  net_minor: number;
  accrual_entry_id: string | null;
  settle_entry_id: string | null;
  last_error: string | null;
}

export interface PeriodPair {
  period: string;
  docs: PeriodDocView[];
}

export interface PeriodStatus {
  period: string;
  licensor_status: DocStatus;
  licensee_status: DocStatus;
  gross_minor: number;
  wht_minor: number;
  net_minor: number;
}

export interface PairView {
  agreement_id: string;
  group_id: string;
  licensor: CompanyRef;
  licensee: CompanyRef;
  currency: string;
  receivable_minor: number;
  payable_minor: number;
  balance_difference_minor: number;
  income_minor: number;
  expense_minor: number;
  income_difference_minor: number;
  status: 'balanced' | 'difference';
  open_periods: string[];
  periods: PeriodStatus[];
}

export interface MemberFigures {
  company: CompanyRef;
  kind: GroupKind;
  ownership_bp: number;
  currency: string;
  revenue_minor: number;
  expenses_minor: number;
  net_income_minor: number;
  assets_minor: number;
  liabilities_minor: number;
  equity_minor: number;
  balanced: boolean;
}

export interface GroupViewOutput {
  group_id: string;
  name: string;
  period: string;
  as_of: string;
  members: MemberFigures[];
  consolidated: string;
}

export interface AgreementCreateInput {
  group_id: string;
  licensor_company_id: string;
  licensee_company_id: string;
  kind: AgreementKind;
  basis: AgreementBasis;
  rate_bp?: number | null;
  fixed_minor?: number | null;
  currency: string;
  treaty_override_bp?: number | null;
  notes?: string | null;
  licensor: AccountsInput;
  licensee: AccountsInput;
}

export interface AgreementUpdateInput {
  kind: AgreementKind;
  basis: AgreementBasis;
  rate_bp?: number | null;
  fixed_minor?: number | null;
  currency: string;
  treaty_override_bp?: number | null;
  notes?: string | null;
  licensor: AccountsInput;
  licensee: AccountsInput;
}

export interface AgreementDetail {
  agreement: AgreementView;
  periods: PeriodDocView[];
}

export interface PeriodsOutput {
  agreement_id: string;
  periods: PeriodPair[];
}

export interface ReconciliationOutput {
  group_id: string;
  pairs: PairView[];
}

export function groupsList(client: ClientConfig): Promise<GroupView[]> {
  return request<{ groups: GroupView[] }>(client, { method: 'GET', path: '/v1/groups' }).then((body) => body.groups);
}

/**
 * Every group the person may see across the given companies: each group once.
 * The server answers each company's list with the groups that company belongs
 * to, filtered to the companies the person may see, so this adds no access.
 * A company whose list fails contributes nothing.
 */
export async function groupsAcrossCompanies(client: ClientConfig, companyIds: readonly string[]): Promise<GroupView[]> {
  const lists = await Promise.all(
    companyIds.map((company) => groupsList({ ...client, company }).catch((): GroupView[] => [])),
  );
  const byId = new Map<string, GroupView>();
  for (const list of lists) for (const group of list) if (!byId.has(group.id)) byId.set(group.id, group);
  return [...byId.values()].sort((a, b) => a.name.localeCompare(b.name) || a.id.localeCompare(b.id));
}

export function groupsCreate(client: ClientConfig, input: { name: string; members: MemberInput[] }): Promise<GroupView> {
  return request<GroupView>(client, { method: 'POST', path: '/v1/groups', body: input });
}

export function groupsUpdate(
  client: ClientConfig,
  id: string,
  input: { name?: string; members?: MemberInput[]; logo_document_id?: string; clear_logo?: boolean },
): Promise<GroupView> {
  return request<GroupView>(client, { method: 'POST', path: `/v1/groups/${encodeURIComponent(id)}/update`, body: input });
}

export function agreementsList(client: ClientConfig): Promise<AgreementView[]> {
  return request<{ agreements: AgreementView[] }>(client, { method: 'GET', path: '/v1/agreements' }).then(
    (body) => body.agreements,
  );
}

export function agreementGet(client: ClientConfig, id: string): Promise<AgreementDetail> {
  return request<AgreementDetail>(client, { method: 'GET', path: `/v1/agreements/${encodeURIComponent(id)}` });
}

export function agreementCreate(
  client: ClientConfig,
  input: AgreementCreateInput,
): Promise<{ licensor: AgreementView; licensee: AgreementView }> {
  return request(client, { method: 'POST', path: '/v1/agreements', body: input });
}

export function agreementUpdate(client: ClientConfig, id: string, input: AgreementUpdateInput): Promise<AgreementView> {
  return request<AgreementView>(client, { method: 'POST', path: `/v1/agreements/${encodeURIComponent(id)}/update`, body: input });
}

export function agreementTerminate(client: ClientConfig, id: string): Promise<AgreementView> {
  return request<AgreementView>(client, { method: 'POST', path: `/v1/agreements/${encodeURIComponent(id)}/terminate` });
}

/** Drafts the periods (both sides). Idempotent: a period already drafted is returned as it is. */
export function runPeriods(client: ClientConfig, id: string, periods: string[]): Promise<PeriodsOutput> {
  return request<PeriodsOutput>(client, {
    method: 'POST',
    path: `/v1/agreements/${encodeURIComponent(id)}/run-period`,
    body: { periods },
  });
}

/** Posts both sides of each period, or neither. A failed side is reported in the response's error (422). */
export function postPeriods(client: ClientConfig, id: string, periods: string[]): Promise<PeriodsOutput> {
  return request<PeriodsOutput>(client, {
    method: 'POST',
    path: `/v1/agreements/${encodeURIComponent(id)}/post-period`,
    body: { periods },
  });
}

export function settlePeriods(client: ClientConfig, id: string, periods: string[]): Promise<PeriodsOutput> {
  return request<PeriodsOutput>(client, {
    method: 'POST',
    path: `/v1/agreements/${encodeURIComponent(id)}/settle-period`,
    body: { periods },
  });
}

export function intercompanyReconciliation(client: ClientConfig, groupId: string): Promise<ReconciliationOutput> {
  return request<ReconciliationOutput>(client, { method: 'GET', path: `/v1/groups/${encodeURIComponent(groupId)}/reconciliation` });
}

export function groupView(client: ClientConfig, groupId: string, period: string, asOf: string): Promise<GroupViewOutput> {
  return request<GroupViewOutput>(client, {
    method: 'GET',
    path: `/v1/groups/${encodeURIComponent(groupId)}/view`,
    query: { period, as_of: asOf },
  });
}
