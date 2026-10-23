/**
 * Pure helpers for the group screens: who may do what (the server enforces it
 * too), ownership in basis points, the withholding display, the entries a
 * period posts (shown as a preview, computed from the amounts the server
 * returned), the period timeline, and the reconciliation's open month. No
 * money is computed here beyond sums of the server's minor-unit amounts.
 */
import type { MessageKey } from '../i18n';
import type {
  AgreementBasis,
  AgreementKind,
  DocStatus,
  GroupView,
  PairView,
  PeriodDocView,
  PeriodPair,
  Side,
  WhtBasis,
} from '../api/groups';
import { parseMajorToMinor } from './money';

export const MAX_BP = 10_000;

/** The company roles that may change the settings of an agreement or a group. */
export function canChangeSettings(role: string | null | undefined): boolean {
  return role === 'owner' || role === 'admin';
}

/** The roles that may run and post a period. */
export function canPostPeriods(role: string | null | undefined): boolean {
  return role === 'owner' || role === 'admin' || role === 'accountant';
}

/** The roles that may settle a period (money out). */
export function canMoveMoney(role: string | null | undefined): boolean {
  return role === 'owner' || role === 'admin';
}

export function isOwner(role: string | null | undefined): boolean {
  return role === 'owner';
}

/** A company the person belongs to, as the group screens see it. */
export interface MemberCompany {
  id: string;
  name: string;
  role: string;
}

/**
 * Why a group cannot be created or changed with these companies, or null when
 * it can. The group rule: the person must be an owner of every member, and
 * there are at least two members.
 */
export function groupMembershipBlocker(
  memberIds: readonly string[],
  companies: readonly MemberCompany[],
): 'fewer_than_two' | 'not_owner' | 'unknown_company' | null {
  if (memberIds.length < 2) return 'fewer_than_two';
  for (const id of memberIds) {
    const company = companies.find((item) => item.id === id);
    if (!company) return 'unknown_company';
    if (!isOwner(company.role)) return 'not_owner';
  }
  return null;
}

/** Converts a percentage the person typed ("62.5", "100") into basis points; null when it is not a valid share. */
export function parsePercentToBp(input: string): number | null {
  const text = input.trim().replace(',', '.');
  if (!/^\d{1,3}(\.\d{1,2})?$/.test(text)) return null;
  const bp = Math.round(Number(text) * 100);
  return bp >= 1 && bp <= MAX_BP ? bp : null;
}

/** Basis points as a percentage string for the locale ("62.5", "62,5"), with at most two decimals. */
export function formatBpPercent(bp: number, locale: string): string {
  return new Intl.NumberFormat(locale, { minimumFractionDigits: 0, maximumFractionDigits: 2 }).format(bp / 100);
}

/** A rate in basis points as a percentage with the locale's separator. */
export function formatRateBp(bp: number | null | undefined, locale: string): string {
  return bp === null || bp === undefined ? '-' : `${formatBpPercent(bp, locale)}%`;
}

/** The percentage an agreement of this basis charges, as the chip reads it. */
export function basisText(kind: 'pct_of_revenue_bp' | 'fixed_monthly' | 'cost_plus_bp'): MessageKey {
  switch (kind) {
    case 'pct_of_revenue_bp':
      return 'groups.basis.pct_of_revenue';
    case 'fixed_monthly':
      return 'groups.basis.fixed_monthly';
    case 'cost_plus_bp':
      return 'groups.basis.cost_plus';
  }
}

/** The periods from `from` to `to` inclusive, each `YYYY-MM`. Empty when `to` is before `from`. */
export function periodsBetween(from: string, to: string, limit = 36): string[] {
  const parse = (value: string) => {
    const match = /^(\d{4})-(\d{2})$/.exec(value);
    return match ? Number(match[1]) * 12 + (Number(match[2]) - 1) : null;
  };
  const start = parse(from);
  const end = parse(to);
  if (start === null || end === null || end < start) return [];
  const out: string[] = [];
  for (let index = start; index <= end && out.length < limit; index += 1) {
    const year = Math.floor(index / 12);
    const month = (index % 12) + 1;
    out.push(`${year}-${String(month).padStart(2, '0')}`);
  }
  return out;
}

/** The message key and values for an agreement's withholding, as the detail page shows it. */
export function whtDisplay(input: {
  wht_bp: number;
  wht_basis: WhtBasis;
  treaty_override_bp: number | null;
}): { key: MessageKey; percent?: string } {
  switch (input.wht_basis) {
    case 'pack':
      return { key: 'groups.wht.pack' };
    case 'treaty_override':
      return { key: 'groups.wht.treaty_override' };
    case 'resident':
      return { key: 'groups.wht.resident' };
    case 'no_rate_on_file':
      return { key: 'groups.wht.no_rate_on_file' };
    case 'not_applicable':
      return { key: 'groups.wht.not_applicable' };
  }
}

/**
 * The withholding rate the form expects before the server answers: the
 * treaty override when one is entered, else the payer's pack rate, else 0
 * when both companies are in one country (the payee is resident). Null when
 * the pack has no rate for the payer's country: the server then says so.
 */
export function expectedWhtBp(input: {
  licenseeCountry: string;
  licensorCountry: string;
  packRateBp: number | null;
  treatyOverrideBp: number | null;
}): { bp: number; basis: WhtBasis } {
  if (input.licenseeCountry && input.licenseeCountry === input.licensorCountry) return { bp: 0, basis: 'resident' };
  if (input.treatyOverrideBp !== null) return { bp: input.treatyOverrideBp, basis: 'treaty_override' };
  if (input.packRateBp !== null) return { bp: input.packRateBp, basis: 'pack' };
  return { bp: 0, basis: 'no_rate_on_file' };
}

/** One line of a preview: the role the account plays, its side and the amount in minor units. */
export interface PreviewLine {
  role: 'ic_receivable' | 'royalty_income' | 'royalty_expense' | 'ic_payable' | 'wht_payable' | 'bank' | 'wht_receivable';
  side: 'debit' | 'credit';
  minor: number;
}

export interface PreviewSide {
  period: string;
  status: DocStatus | null;
  currency: string;
  lines: PreviewLine[];
  debitTotal: number;
  creditTotal: number;
  balanced: boolean;
  /** The amount the licensor or licensee holds or pays: net for the licensee, gross for the licensor. */
  headline: number;
}

function sideTotals(lines: PreviewLine[]): { debitTotal: number; creditTotal: number } {
  return lines.reduce(
    (acc, line) =>
      line.side === 'debit'
        ? { ...acc, debitTotal: acc.debitTotal + line.minor }
        : { ...acc, creditTotal: acc.creditTotal + line.minor },
    { debitTotal: 0, creditTotal: 0 },
  );
}

function previewOf(period: string, doc: PeriodDocView | undefined, lines: PreviewLine[], headline: number): PreviewSide {
  const totals = sideTotals(lines);
  return {
    period,
    status: doc?.status ?? null,
    currency: doc?.currency ?? '',
    lines: lines.filter((line) => line.minor !== 0),
    ...totals,
    balanced: totals.debitTotal === totals.creditTotal,
    headline,
  };
}

/**
 * The accrual entries of one period, both sides: the licensor invoices
 * (receivable gross against royalty income) and the licensee books the bill
 * (royalty expense gross against the intercompany payable net and the
 * withholding payable). Amounts come from the server's period documents.
 */
export function accrualPreview(
  licensor: PeriodDocView | undefined,
  licensee: PeriodDocView | undefined,
  period: string,
): { licensor: PreviewSide; licensee: PreviewSide } {
  const gross = licensor?.gross_minor ?? licensee?.gross_minor ?? 0;
  const wht = licensee?.wht_minor ?? 0;
  const net = licensee?.net_minor ?? gross - wht;
  return {
    licensor: previewOf(period, licensor, [
      { role: 'ic_receivable', side: 'debit', minor: gross },
      { role: 'royalty_income', side: 'credit', minor: gross },
    ], gross),
    licensee: previewOf(period, licensee, [
      { role: 'royalty_expense', side: 'debit', minor: gross },
      { role: 'ic_payable', side: 'credit', minor: net },
      { role: 'wht_payable', side: 'credit', minor: wht },
    ], net),
  };
}

/** The settlement entries: the licensee pays the net, the licensor receives it and holds the withholding as a receivable. */
export function settlementPreview(
  licensor: PeriodDocView | undefined,
  licensee: PeriodDocView | undefined,
  period: string,
): { licensor: PreviewSide; licensee: PreviewSide } {
  const gross = licensor?.gross_minor ?? licensee?.gross_minor ?? 0;
  const wht = licensee?.wht_minor ?? 0;
  const net = licensee?.net_minor ?? gross - wht;
  return {
    licensor: previewOf(period, licensor, [
      { role: 'bank', side: 'debit', minor: net },
      { role: 'wht_receivable', side: 'debit', minor: wht },
      { role: 'ic_receivable', side: 'credit', minor: gross },
    ], net),
    licensee: previewOf(period, licensee, [
      { role: 'ic_payable', side: 'debit', minor: net },
      { role: 'bank', side: 'credit', minor: net },
    ], net),
  };
}

/** The state of one month across both sides, for the timeline. `none` when neither side is drafted. */
export type TimelineState = 'none' | 'draft' | 'posted' | 'settled' | 'failed';

export interface TimelineCell {
  period: string;
  state: TimelineState;
  licensor: PeriodDocView | undefined;
  licensee: PeriodDocView | undefined;
}

/** The months with documents, oldest first. A failed side fails the month; both settled is settled. */
export function timelineCells(docs: readonly PeriodDocView[]): TimelineCell[] {
  const byPeriod = new Map<string, PeriodDocView[]>();
  for (const doc of docs) byPeriod.set(doc.period, [...(byPeriod.get(doc.period) ?? []), doc]);
  return [...byPeriod.keys()]
    .sort()
    .map((period) => {
      const pair = byPeriod.get(period) ?? [];
      const licensor = pair.find((doc) => doc.side === 'licensor');
      const licensee = pair.find((doc) => doc.side === 'licensee');
      return { period, state: combinedState(licensor, licensee), licensor, licensee };
    });
}

function combinedState(licensor: PeriodDocView | undefined, licensee: PeriodDocView | undefined): TimelineState {
  const statuses = [licensor?.status, licensee?.status].filter((value): value is DocStatus => value !== undefined);
  if (statuses.includes('failed')) return 'failed';
  if (statuses.length > 0 && statuses.every((status) => status === 'settled')) return 'settled';
  if (statuses.includes('posted') || statuses.includes('settled')) return 'posted';
  if (statuses.includes('draft')) return 'draft';
  return 'none';
}

/** The next action a period offers, from its state. Settling needs both sides posted; posting needs both drafted. */
export function nextAction(cell: TimelineCell): 'post' | 'settle' | 'retry' | 'none' {
  if (cell.state === 'failed') return 'retry';
  if (cell.state === 'draft') return 'post';
  if (cell.state === 'posted') return 'settle';
  return 'none';
}

/** The document rows of a period, both sides, for the preview. */
export function docsOf(pair: PeriodPair | undefined): { licensor: PeriodDocView | undefined; licensee: PeriodDocView | undefined } {
  return {
    licensor: pair?.docs.find((doc) => doc.side === 'licensor'),
    licensee: pair?.docs.find((doc) => doc.side === 'licensee'),
  };
}

/** Whether a reconciled pair has a difference to show: a balance or an income difference. */
export function pairHasDifference(pair: PairView): boolean {
  return pair.balance_difference_minor !== 0 || pair.income_difference_minor !== 0;
}

/** The open month the reconciliation shows: the most recent open period, or null when all are settled. */
export function openMonth(pair: PairView): string | null {
  return pair.open_periods.length > 0 ? [...pair.open_periods].sort().at(-1) ?? null : null;
}

/** Account type classes an agreement side may name, by role. The server checks the same classes. */
export const ACCOUNT_TYPES: Record<'royalty' | 'ic' | 'wht' | 'bank', Record<Side, string[]>> = {
  royalty: { licensor: ['income'], licensee: ['expense'] },
  ic: { licensor: ['asset'], licensee: ['liability'] },
  wht: { licensor: ['asset'], licensee: ['liability'] },
  bank: { licensor: ['asset'], licensee: ['asset'] },
};

export interface ChartAccount {
  number: string;
  account_type: string;
  is_postable: boolean;
  archived: boolean;
}

/** The accounts a picker offers for one role of one side: the right type, live and postable. */
export function accountsFor<T extends ChartAccount>(accounts: readonly T[], role: keyof typeof ACCOUNT_TYPES, side: Side): T[] {
  const types = ACCOUNT_TYPES[role][side];
  return accounts.filter((account) => types.includes(account.account_type) && account.is_postable && !account.archived);
}

/** The groups the nav needs: the person belongs to a group, or owns two or more companies. */
export function groupNavVisible(groups: readonly GroupView[], companies: readonly MemberCompany[]): boolean {
  return groups.length > 0 || companies.filter((company) => isOwner(company.role)).length >= 2;
}

/** The companies of a group the person may see, as their names: the others are shown as hidden. */
export function groupCompanyIds(group: GroupView): string[] {
  return group.members.map((member) => member.company_id);
}

/** The last day of a month as `YYYY-MM-DD` (the balance sheet date of a month). */
export function monthEnd(period: string): string {
  const match = /^(\d{4})-(\d{2})$/.exec(period);
  if (!match) return period;
  const last = new Date(Date.UTC(Number(match[1]), Number(match[2]), 0));
  return last.toISOString().slice(0, 10);
}

/** The account numbers one side has chosen in the form. */
export interface SideAccounts {
  royalty: string;
  ic: string;
  wht: string;
  bank: string;
  unit: string;
}

/** Problems the form found before the server did: the message key, or null when the form may be sent. */
export function formProblem(input: {
  groupId: string;
  licensorId: string;
  licenseeId: string;
  licensorRole: string;
  licenseeRole: string;
  licensorCurrency: string;
  licenseeCurrency: string;
  kind: AgreementKind;
  basis: AgreementBasis;
  ratePercent: string;
  fixedText: string;
  currency: string;
  treatyPercent: string;
  licensorAccounts: SideAccounts;
  licenseeAccounts: SideAccounts;
  licensorUnitsRequired: boolean;
  licenseeUnitsRequired: boolean;
}): MessageKey | null {
  if (input.groupId === '') return 'agreements.err.group';
  if (input.licensorId === '' || input.licenseeId === '') return 'agreements.err.companies';
  if (input.licensorId === input.licenseeId) return 'agreements.err.same_company';
  if (!canChangeSettings(input.licensorRole) || !canChangeSettings(input.licenseeRole)) return 'agreements.err.settings_both';
  if (input.licensorCurrency !== input.licenseeCurrency) return 'agreements.err.currency_mismatch';
  if (input.currency !== input.licensorCurrency) return 'agreements.err.currency_mismatch';
  if (input.basis === 'fixed_monthly') {
    const fixed = parseMajorToMinor(input.fixedText, input.currency);
    if (fixed === null || fixed <= 0n) return 'agreements.err.fixed';
  } else if (parsePercentToBp(input.ratePercent) === null) {
    return 'agreements.err.rate';
  }
  if (input.treatyPercent.trim() !== '' && parsePercentToBp(input.treatyPercent) === null) return 'agreements.err.treaty';
  for (const side of [input.licensorAccounts, input.licenseeAccounts]) {
    if (side.royalty === '' || side.ic === '' || side.bank === '') return 'agreements.err.accounts';
  }
  if (input.licensorUnitsRequired && input.licensorAccounts.unit === '') return 'agreements.err.unit';
  if (input.licenseeUnitsRequired && input.licenseeAccounts.unit === '') return 'agreements.err.unit';
  return null;
}

