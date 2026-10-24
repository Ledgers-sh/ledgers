import type { ReceivableOpenItem } from '../api/bank';

/** One open item a receipt settles, and how much of it the receipt takes. */
export interface Settlement {
  item: ReceivableOpenItem;
  settle: bigint;
}

/**
 * How a receipt settles a customer's open items, oldest first: each item takes
 * what it still owes until the receipt runs out. Whatever is left over is the
 * customer's on-account credit. Mirrors the server's allocation, so the screen
 * shows what the posting will do before the person confirms it.
 */
export function receiptSettlement(amount: bigint, items: ReceivableOpenItem[]): { settlements: Settlement[]; onAccount: bigint } {
  let left = amount;
  const settlements: Settlement[] = [];
  for (const item of items) {
    if (left <= 0n) break;
    const owed = BigInt(item.remaining.minor);
    const settle = owed < left ? owed : left;
    if (settle > 0n) {
      settlements.push({ item, settle });
      left -= settle;
    }
  }
  return { settlements, onAccount: left > 0n ? left : 0n };
}
