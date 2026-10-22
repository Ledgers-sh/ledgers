import { describe, expect, it } from 'vitest';
import type { TrendMonth } from '../api/types';
import {
  averageMonthlyExpenses,
  changeOf,
  endBalance,
  formatRunway,
  monthCount,
  monthsIn,
  normaliseRange,
  presetOf,
  presetRange,
  previousRange,
  rollingYear,
  runwayMonths,
  sumOf,
} from './trends';

const EUR = 'EUR';
const amount = (minor: number) => ({ minor: String(minor), currency: EUR });

function month(period: string, revenue: number, expenses: number, cash = 0, receivables = 0, payables = 0): TrendMonth {
  return {
    period,
    revenue: amount(revenue),
    expenses: amount(expenses),
    net_result: amount(revenue - expenses),
    cash: amount(cash),
    receivables: amount(receivables),
    payables: amount(payables),
  };
}

describe('change against the previous period', () => {
  it('is a percentage of the previous value, one decimal', () => {
    expect(changeOf(12_400n, 10_000n)).toEqual({ direction: 'up', percent: 24 });
    expect(changeOf(9_700n, 10_000n)).toEqual({ direction: 'down', percent: 3 });
    expect(changeOf(10_033n, 10_000n)).toEqual({ direction: 'up', percent: 0.3 });
  });

  it('measures against the size of a negative previous value', () => {
    // From a loss of 1,000 to a loss of 500 is an improvement of 50 percent.
    expect(changeOf(-500n, -1_000n)).toEqual({ direction: 'up', percent: 50 });
    expect(changeOf(-2_000n, -1_000n)).toEqual({ direction: 'down', percent: 100 });
  });

  it('has no percentage from zero: new when there is now something, flat when nothing', () => {
    expect(changeOf(5_000n, 0n)).toEqual({ direction: 'new', percent: null });
    expect(changeOf(0n, 0n)).toEqual({ direction: 'flat', percent: null });
    expect(changeOf(7n, 7n)).toEqual({ direction: 'flat', percent: null });
  });

  it('is exact on large amounts (no float rounding of minor units)', () => {
    const big = 9_007_199_254_740_993n;
    expect(changeOf(big + 1n, big)).toEqual({ direction: 'up', percent: 0 });
    expect(changeOf(big * 2n, big)).toEqual({ direction: 'up', percent: 100 });
  });
});

describe('runway', () => {
  it('is cash divided by the average monthly expenses, one decimal', () => {
    // 1,500.00 of cash against 300.00 a month covers five months.
    expect(runwayMonths(150_000n, 30_000n)).toBe(5);
    // 1,000.00 against 300.00 covers 3.3 months.
    expect(runwayMonths(100_000n, 30_000n)).toBe(3.3);
  });

  it('is null without expenses, since the runway would be unbounded', () => {
    expect(runwayMonths(150_000n, 0n)).toBeNull();
  });

  it('is zero with no positive cash, however small the expenses', () => {
    expect(runwayMonths(0n, 30_000n)).toBe(0);
    expect(runwayMonths(-40_000n, 30_000n)).toBe(0);
  });

  it('averages expenses over the months in the window', () => {
    const months = [month('2026-01', 0, 30_000), month('2026-02', 0, 60_000), month('2026-03', 0, 0)];
    expect(averageMonthlyExpenses(months)).toBe(30_000n);
    expect(averageMonthlyExpenses([])).toBe(0n);
  });

  it('shows whole months from ten up and one decimal below', () => {
    expect(formatRunway(3.3)).toBe('3.3');
    expect(formatRunway(9.9)).toBe('9.9');
    expect(formatRunway(12.8)).toBe('12');
  });
});

describe('period ranges', () => {
  it('lists every month of a range, across a year boundary', () => {
    expect(monthsIn({ from: '2025-11', to: '2026-02' })).toEqual(['2025-11', '2025-12', '2026-01', '2026-02']);
    expect(monthCount({ from: '2026-03', to: '2026-03' })).toBe(1);
  });

  it('compares with the same number of months just before', () => {
    expect(previousRange({ from: '2026-01', to: '2026-10' })).toEqual({ from: '2025-03', to: '2025-12' });
    expect(previousRange({ from: '2026-10', to: '2026-10' })).toEqual({ from: '2026-09', to: '2026-09' });
  });

  it('gives the twelve months ending at a month', () => {
    expect(rollingYear('2026-10')).toEqual({ from: '2025-11', to: '2026-10' });
  });

  it('resolves presets against today, with quarter and year to date', () => {
    const today = new Date(2026, 9, 22); // 22 October 2026
    expect(presetRange('thisMonth', today)).toEqual({ from: '2026-10', to: '2026-10' });
    expect(presetRange('lastMonth', today)).toEqual({ from: '2026-09', to: '2026-09' });
    expect(presetRange('thisQuarter', today)).toEqual({ from: '2026-10', to: '2026-10' });
    expect(presetRange('thisYear', today)).toEqual({ from: '2026-01', to: '2026-10' });
    expect(presetRange('last12', today)).toEqual({ from: '2025-11', to: '2026-10' });
    const february = new Date(2026, 1, 3);
    expect(presetRange('thisQuarter', february)).toEqual({ from: '2026-01', to: '2026-02' });
  });

  it('recognises a preset, or calls the range custom', () => {
    const today = new Date(2026, 9, 22);
    expect(presetOf({ from: '2026-01', to: '2026-10' }, today)).toBe('thisYear');
    expect(presetOf({ from: '2026-02', to: '2026-05' }, today)).toBe('custom');
  });

  it('puts a backwards range the right way round', () => {
    expect(normaliseRange({ from: '2026-05', to: '2026-02' })).toEqual({ from: '2026-02', to: '2026-05' });
  });
});

describe('window sums and balances', () => {
  const months = [
    month('2026-01', 100_000, 30_000, 70_000, 0, 0),
    month('2026-02', 80_000, 20_000, 70_000, 80_000, 20_000),
    month('2026-03', 0, 0, 150_000, 0, 20_000),
  ];

  it('sums flows across the window and reads balances at its end', () => {
    expect(sumOf(months, 'revenue')).toBe(180_000n);
    expect(sumOf(months, 'expenses')).toBe(50_000n);
    expect(sumOf(months, 'net_result')).toBe(130_000n);
    expect(endBalance(months, 'cash')).toBe(150_000n);
    expect(endBalance(months, 'receivables')).toBe(0n);
    expect(endBalance(months, 'payables')).toBe(20_000n);
    expect(endBalance([], 'cash')).toBe(0n);
  });
});
