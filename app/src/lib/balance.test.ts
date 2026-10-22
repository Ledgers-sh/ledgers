import { describe, expect, it } from 'vitest';
import { checkBalance } from './balance';

describe('checkBalance', () => {
  it('is balanced when debits equal credits with at least two lines', () => {
    const result = checkBalance([
      { side: 'debit', minor: 10000n },
      { side: 'credit', minor: 10000n },
    ]);
    expect(result.balanced).toBe(true);
    expect(result.difference).toBe(0n);
    expect(result.debit).toBe(10000n);
  });

  it('reports the difference when unequal', () => {
    const result = checkBalance([
      { side: 'debit', minor: 10000n },
      { side: 'credit', minor: 9950n },
    ]);
    expect(result.balanced).toBe(false);
    expect(result.difference).toBe(50n);
  });

  it('is not balanced with fewer than two filled lines', () => {
    expect(checkBalance([{ side: 'debit', minor: 500n }, { side: 'credit', minor: null }]).balanced).toBe(false);
    expect(checkBalance([]).balanced).toBe(false);
  });

  it('is not balanced while any amount is invalid', () => {
    const result = checkBalance([
      { side: 'debit', minor: 500n },
      { side: 'credit', minor: 500n },
      { side: 'debit', minor: null },
    ]);
    expect(result.hasInvalid).toBe(true);
    expect(result.balanced).toBe(false);
  });

  it('ignores zero lines in the filled count', () => {
    const result = checkBalance([
      { side: 'debit', minor: 500n },
      { side: 'credit', minor: 500n },
      { side: 'debit', minor: 0n },
    ]);
    expect(result.filled).toBe(2);
    expect(result.balanced).toBe(true);
  });
});
