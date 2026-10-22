/** Client-side balance check for the New entry form. The server re-checks every entry. */

export interface CheckLine {
  side: 'debit' | 'credit';
  /** BigInt minor units; null when the amount is not yet valid. */
  minor: bigint | null;
}

export interface BalanceResult {
  debit: bigint;
  credit: bigint;
  /** debit minus credit: positive means the debits are larger. */
  difference: bigint;
  /** Lines with an amount that parsed. */
  filled: number;
  /** True when at least one line has invalid text in its amount field. */
  hasInvalid: boolean;
  balanced: boolean;
}

export function checkBalance(lines: CheckLine[]): BalanceResult {
  let debit = 0n;
  let credit = 0n;
  let filled = 0;
  let hasInvalid = false;
  for (const line of lines) {
    if (line.minor === null) {
      hasInvalid = true;
      continue;
    }
    if (line.minor === 0n) continue;
    filled += 1;
    if (line.side === 'debit') debit += line.minor;
    else credit += line.minor;
  }
  const difference = debit - credit;
  return {
    debit,
    credit,
    difference,
    filled,
    hasInvalid,
    balanced: !hasInvalid && filled >= 2 && difference === 0n,
  };
}
