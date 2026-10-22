/**
 * Pure helpers for the approvals inbox (issue #33): which rows may be bulk
 * approved, what the badge says, how amounts and times read, and how a failed
 * decision is classified. No DOM, no network, so the rules are unit tested.
 */
import { ApiError, buildUrl } from '../api/client';
import type { ApprovalView } from '../api/approvals';
import { formatMinor } from './money';

/** Why a selection cannot be bulk approved. The screen words it in the reader's language. */
export type PlanMessage = { kind: 'high-risk' } | { kind: 'mixed'; currencies: string[] };

/** Only a pending low-risk request can be bulk approved. */
export function isBulkEligible(item: Pick<ApprovalView, 'status' | 'risk'>): boolean {
  return item.status === 'pending' && item.risk === 'low';
}

/** The currency codes an approval's amount is in, sorted. */
export function currenciesOf(item: Pick<ApprovalView, 'amount'>): string[] {
  return Object.keys(item.amount ?? {}).sort();
}

export interface BulkPlan {
  /** True when the bulk button may be enabled. */
  canApprove: boolean;
  /** How many of the selected rows the plan covers. */
  count: number;
  /** The one currency of the selection, or null when the selection has no amount. */
  currency: string | null;
  /** Why the bulk button is off, or null. */
  message: PlanMessage | null;
}

/**
 * Decides whether the selected rows can be approved together. Refuses when any
 * selected row is high-risk (the server refuses too) and when the rows span
 * more than one currency, so the bulk approve never offers a mixed selection.
 */
export function bulkPlan(items: readonly ApprovalView[], selectedIds: ReadonlySet<string>): BulkPlan {
  const selected = items.filter((item) => selectedIds.has(item.id));
  const count = selected.length;
  if (count === 0) return { canApprove: false, count, currency: null, message: null };
  if (selected.some((item) => !isBulkEligible(item))) {
    return { canApprove: false, count, currency: null, message: { kind: 'high-risk' } };
  }
  const currencies = [...new Set(selected.flatMap(currenciesOf))].sort();
  if (currencies.length > 1) {
    return {
      canApprove: false,
      count,
      currency: null,
      message: { kind: 'mixed', currencies },
    };
  }
  return { canApprove: true, count, currency: currencies[0] ?? null, message: null };
}

/** The sum of the selected rows' amounts in their one currency, as minor units, or null when not computable. */
export function bulkTotalMinor(items: readonly ApprovalView[], selectedIds: ReadonlySet<string>, currency: string): string | null {
  let total = 0n;
  for (const item of items) {
    if (!selectedIds.has(item.id)) continue;
    const value = item.amount?.[currency];
    if (value === undefined) continue;
    if (!/^-?\d+$/.test(value)) return null;
    total += BigInt(value);
  }
  return total.toString();
}

/** The badge on the nav item: null when nothing is pending, "99+" above 99. */
export function badgeText(count: number | null | undefined): string | null {
  if (count === null || count === undefined || count <= 0) return null;
  return count > 99 ? '99+' : String(count);
}

/** One amount as people read it: minor units formatted in the currency's decimals; other strings shown as sent. */
export function displayAmount(value: string, currency: string, locale = 'en-US'): string {
  if (/^-?\d+$/.test(value) && /^[A-Z]{3}$/.test(currency)) {
    return `${formatMinor(value, currency, locale)} ${currency}`;
  }
  return `${value} ${currency}`.trim();
}

/** An approval's amount, one part per currency, joined. "—" when there is no amount. */
export function amountText(amount: Record<string, string> | null | undefined, locale = 'en-US'): string {
  const parts = Object.entries(amount ?? {}).map(([currency, value]) => displayAmount(value, currency, locale));
  return parts.length > 0 ? parts.join(' + ') : '—';
}

const UNITS: [Intl.RelativeTimeFormatUnit, number][] = [
  ['day', 86_400_000],
  ['hour', 3_600_000],
  ['minute', 60_000],
];

/** "2 hours ago" / "in 3 days" for an RFC 3339 time against `now` (milliseconds), in the given locale. */
export function relativeTime(iso: string, now: number, locale = 'en'): string {
  const target = Date.parse(iso);
  if (Number.isNaN(target)) return '';
  const diff = target - now;
  const rtf = new Intl.RelativeTimeFormat(locale, { numeric: 'auto' });
  for (const [unit, size] of UNITS) {
    if (Math.abs(diff) >= size) return rtf.format(Math.round(diff / size), unit);
  }
  return rtf.format(Math.round(diff / 1000), 'second');
}

/**
 * When an approval expires: whether it is still open, and how far away the
 * time is in the locale ("in 2 hours" / "2 hours ago"). Null for a bad time.
 */
export function expiry(expiresAt: string, now: number, locale = 'en'): { open: boolean; when: string } | null {
  const target = Date.parse(expiresAt);
  if (Number.isNaN(target)) return null;
  return { open: target > now, when: relativeTime(expiresAt, now, locale) };
}

/** A timestamp for the history table: date and time in the reader's locale. */
export function formatTimestamp(iso: string, locale = 'en-US'): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return new Intl.DateTimeFormat(locale, { dateStyle: 'medium', timeStyle: 'short' }).format(date);
}

/** The first twelve characters of the request hash, with an ellipsis. */
export function shortHash(hash: string): string {
  return hash.length > 12 ? `${hash.slice(0, 12)}…` : hash;
}

/** Where the browser goes to sign in again, returning to this approval (contract: prompt=login, return_to). */
export function reauthUrl(baseUrl: string, approvalId: string): string {
  return buildUrl(baseUrl, '/v1/auth/login', {
    prompt: 'login',
    return_to: `/approvals/${encodeURIComponent(approvalId)}`,
  });
}

export type DecisionFailure =
  | { kind: 'reauth' }
  | { kind: 'stale'; reason: 'not-pending' | 'expired' }
  | { kind: 'refused'; detail: string | null }
  | { kind: 'other' };

/** Sorts a failed decision: sign in again, the item changed under us, the selection was refused, or other. */
export function classifyDecisionError(error: unknown): DecisionFailure {
  if (!(error instanceof ApiError)) return { kind: 'other' };
  switch (error.slug) {
    case 'reauth-required':
      return { kind: 'reauth' };
    case 'approval-not-pending':
      return { kind: 'stale', reason: 'not-pending' };
    case 'approval-expired':
      return { kind: 'stale', reason: 'expired' };
    case 'bulk-selection-refused':
      return { kind: 'refused', detail: error.detail };
    default:
      return { kind: 'other' };
  }
}

/** The entry a decision posted, read from the command output: its id and the journal key to link to. */
export function resultEntry(result: unknown): { entryId: string; journalKey: string } | null {
  if (!result || typeof result !== 'object') return null;
  const top = result as Record<string, unknown>;
  const entry = (top.entry && typeof top.entry === 'object' ? top.entry : top) as Record<string, unknown>;
  const entryId = typeof entry.entry_id === 'string' && entry.entry_id !== '' ? entry.entry_id : null;
  if (!entryId) return null;
  const external = typeof entry.external_id === 'string' && entry.external_id !== '' ? entry.external_id : null;
  return { entryId, journalKey: external ?? entryId };
}
