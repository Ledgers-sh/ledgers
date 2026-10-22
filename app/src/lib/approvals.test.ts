import { describe, expect, it } from 'vitest';
import { ApiError } from '../api/client';
import type { ApprovalView } from '../api/approvals';
import {
  amountText,
  badgeText,
  bulkPlan,
  bulkTotalMinor,
  classifyDecisionError,
  displayAmount,
  expiryText,
  formatTimestamp,
  isBulkEligible,
  reauthUrl,
  relativeTime,
  resultEntry,
  shortHash,
  BULK_HIGH_RISK_MESSAGE,
  EXPIRED_MESSAGE,
  NOT_PENDING_MESSAGE,
} from './approvals';

function item(overrides: Partial<ApprovalView> & { id: string }): ApprovalView {
  return {
    status: 'pending',
    command: 'entries.post',
    risk: 'low',
    amount: { EUR: '12500' },
    counterparty: 'Halden Works GmbH',
    reason: 'exceeds per-entry limit',
    requested_by: 'key_agent_1',
    created_at: '2026-10-22T08:00:00Z',
    expires_at: '2026-10-22T20:00:00Z',
    decided_at: null,
    decided_by: null,
    decision_reason: null,
    result: null,
    audit_ref: null,
    request_hash: 'sha256:0123456789abcdef0123',
    ...overrides,
  };
}

describe('bulk eligibility', () => {
  it('is eligible only when pending and low risk', () => {
    expect(isBulkEligible(item({ id: 'a', risk: 'low' }))).toBe(true);
    expect(isBulkEligible(item({ id: 'b', risk: 'high' }))).toBe(false);
    expect(isBulkEligible(item({ id: 'c', status: 'approved' }))).toBe(false);
  });
});

describe('bulkPlan', () => {
  const rows = [
    item({ id: 'eur1', amount: { EUR: '10000' } }),
    item({ id: 'eur2', amount: { EUR: '2550' } }),
    item({ id: 'usd1', amount: { USD: '4000' } }),
    item({ id: 'high1', risk: 'high', amount: { EUR: '5000000' } }),
  ];

  it('offers nothing for an empty selection', () => {
    expect(bulkPlan(rows, new Set())).toEqual({ canApprove: false, count: 0, currency: null, message: null });
  });

  it('allows a selection of low-risk rows in one currency and names that currency', () => {
    expect(bulkPlan(rows, new Set(['eur1', 'eur2']))).toEqual({ canApprove: true, count: 2, currency: 'EUR', message: null });
  });

  it('refuses a selection that includes a high-risk row', () => {
    const plan = bulkPlan(rows, new Set(['eur1', 'high1']));
    expect(plan.canApprove).toBe(false);
    expect(plan.message).toBe(BULK_HIGH_RISK_MESSAGE);
  });

  it('refuses a mixed-currency selection with a message naming both currencies', () => {
    const plan = bulkPlan(rows, new Set(['eur1', 'usd1']));
    expect(plan.canApprove).toBe(false);
    expect(plan.message).toBe('The selection mixes EUR and USD. Select one currency at a time.');
  });

  it('ignores ids that are not on screen', () => {
    expect(bulkPlan(rows, new Set(['gone']))).toEqual({ canApprove: false, count: 0, currency: null, message: null });
  });

  it('sums the selected amounts in their currency', () => {
    expect(bulkTotalMinor(rows, new Set(['eur1', 'eur2']), 'EUR')).toBe('12550');
    expect(bulkTotalMinor([item({ id: 'x', amount: { EUR: '1.5' } })], new Set(['x']), 'EUR')).toBeNull();
  });
});

describe('badge and formatting helpers', () => {
  it('hides the badge at zero and caps it at 99+', () => {
    expect(badgeText(0)).toBeNull();
    expect(badgeText(null)).toBeNull();
    expect(badgeText(3)).toBe('3');
    expect(badgeText(120)).toBe('99+');
  });

  it('formats an amount with the currency decimals, and shows an unparsed string as sent', () => {
    expect(displayAmount('12500', 'EUR', 'en-US')).toBe('125.00 EUR');
    expect(displayAmount('1234', 'JPY', 'en-US')).toBe('1,234 JPY');
    expect(displayAmount('12.50', 'EUR', 'en-US')).toBe('12.50 EUR');
  });

  it('joins several currencies and shows a dash without an amount', () => {
    expect(amountText({ EUR: '12500', USD: '400' }, 'en-US')).toBe('125.00 EUR + 4.00 USD');
    expect(amountText({}, 'en-US')).toBe('—');
  });

  it('reads relative times and expiry in words', () => {
    const now = Date.parse('2026-10-22T12:00:00Z');
    expect(relativeTime('2026-10-22T10:00:00Z', now)).toBe('2 hours ago');
    expect(relativeTime('2026-10-22T15:00:00Z', now)).toBe('in 3 hours');
    expect(expiryText('2026-10-22T14:00:00Z', now)).toBe('expires in 2 hours');
    expect(expiryText('2026-10-22T11:30:00Z', now)).toBe('expired 30 minutes ago');
    expect(relativeTime('not a time', now)).toBe('');
  });

  it('formats a decision time and shortens the request hash', () => {
    expect(formatTimestamp('2026-10-22T10:05:00Z', 'en-US')).toContain('2026');
    expect(formatTimestamp('nope', 'en-US')).toBe('nope');
    expect(shortHash('sha256:0123456789abcdef0123')).toBe('sha256:01234…');
    expect(shortHash('short')).toBe('short');
  });
});

describe('reauth and decision errors', () => {
  it('builds the sign-in URL that returns to the approval', () => {
    const url = new URL(reauthUrl('http://127.0.0.1:3000/', 'apr-7'));
    expect(url.origin + url.pathname).toBe('http://127.0.0.1:3000/v1/auth/login');
    expect(url.searchParams.get('prompt')).toBe('login');
    expect(url.searchParams.get('return_to')).toBe('/approvals/apr-7');
  });

  it('classifies each slug the contract names', () => {
    expect(classifyDecisionError(new ApiError('unauthorized', 'x', 401, null, 'reauth-required'))).toEqual({ kind: 'reauth' });
    expect(classifyDecisionError(new ApiError('server', 'x', 409, null, 'approval-not-pending'))).toEqual({ kind: 'stale', message: NOT_PENDING_MESSAGE });
    expect(classifyDecisionError(new ApiError('server', 'x', 409, null, 'approval-expired'))).toEqual({ kind: 'stale', message: EXPIRED_MESSAGE });
    expect(classifyDecisionError(new ApiError('validation', 'x', 422, 'Mixed currencies', 'bulk-selection-refused'))).toEqual({
      kind: 'refused',
      message: 'Mixed currencies',
    });
    expect(classifyDecisionError(new ApiError('validation', 'x', 422, null, 'bulk-selection-refused'))).toEqual({
      kind: 'refused',
      message: 'The server refused this selection. Nothing was approved.',
    });
    expect(classifyDecisionError(new Error('boom'))).toEqual({ kind: 'other' });
  });

  it('reads the entry a decision posted, preferring its external id for the journal link', () => {
    expect(resultEntry({ entry: { entry_id: 'entry-1', external_id: 'ext-1' } })).toEqual({ entryId: 'entry-1', journalKey: 'ext-1' });
    expect(resultEntry({ entry_id: 'entry-2' })).toEqual({ entryId: 'entry-2', journalKey: 'entry-2' });
    expect(resultEntry({ something: 'else' })).toBeNull();
    expect(resultEntry(null)).toBeNull();
  });
});
