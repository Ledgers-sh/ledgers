import { describe, expect, it } from 'vitest';
import type { ReceivableOpenItem } from '../api/bank';
import { receiptSettlement } from './receipt';

function item(id: string, minor: string): ReceivableOpenItem {
  return { id, kind: 'invoice', document: id.toUpperCase(), posting_date: '2026-09-01', due_date: null, remaining: { minor, currency: 'EUR' } };
}

describe('receiptSettlement', () => {
  it('settles the one invoice it pays in full, with nothing on account', () => {
    const { settlements, onAccount } = receiptSettlement(100000n, [item('a', '100000')]);
    expect(settlements.map((s) => [s.item.id, s.settle])).toEqual([['a', 100000n]]);
    expect(onAccount).toBe(0n);
  });

  it('settles the older invoice in full and part-pays the newer', () => {
    const { settlements, onAccount } = receiptSettlement(150000n, [item('older', '100000'), item('newer', '100000')]);
    expect(settlements.map((s) => [s.item.id, s.settle])).toEqual([
      ['older', 100000n],
      ['newer', 50000n],
    ]);
    expect(onAccount).toBe(0n);
  });

  it('leaves the excess on account when the receipt is more than the invoices', () => {
    const { settlements, onAccount } = receiptSettlement(420000n, [item('a', '100000')]);
    expect(settlements.map((s) => s.item.id)).toEqual(['a']);
    expect(onAccount).toBe(320000n);
  });

  it('is all on account when there are no open invoices', () => {
    expect(receiptSettlement(5000n, [])).toEqual({ settlements: [], onAccount: 5000n });
  });
});
