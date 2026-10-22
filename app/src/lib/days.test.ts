import { describe, expect, it } from 'vitest';
import { isDay, isoDay, monthGrid, shiftDay, shiftMonth, todayUtc } from './days';

describe('days', () => {
  it('accepts only real calendar days', () => {
    expect(isDay('2026-02-28')).toBe(true);
    expect(isDay('2026-02-29')).toBe(false);
    expect(isDay('2026-13-01')).toBe(false);
    expect(isDay('2026-10')).toBe(false);
    expect(isDay('')).toBe(false);
  });

  it('moves by days across month and year ends', () => {
    expect(shiftDay('2026-03-01', -1)).toBe('2026-02-28');
    expect(shiftDay('2026-12-31', 1)).toBe('2027-01-01');
    expect(shiftDay('2026-10-05', 7)).toBe('2026-10-12');
  });

  it('moves by months and clamps to a shorter month', () => {
    expect(shiftMonth('2026-01-31', 1)).toBe('2026-02-28');
    expect(shiftMonth('2026-03-15', -1)).toBe('2026-02-15');
    expect(shiftMonth('2026-12-10', 1)).toBe('2027-01-10');
  });

  it('lays a month out Monday-first in whole weeks', () => {
    // October 2026 starts on a Thursday and has 31 days.
    const cells = monthGrid('2026-10-14');
    expect(cells.length % 7).toBe(0);
    expect(cells.slice(0, 3)).toEqual([null, null, null]);
    expect(cells[3]).toBe('2026-10-01');
    expect(cells.filter((cell) => cell !== null)).toHaveLength(31);
    expect(cells.filter((cell) => cell !== null).at(-1)).toBe('2026-10-31');
  });

  it('builds a leap February', () => {
    // February 2028 starts on a Tuesday and has 29 days.
    const days = monthGrid('2028-02-01').filter((cell) => cell !== null);
    expect(days).toHaveLength(29);
    expect(monthGrid('2028-02-01')[1]).toBe('2028-02-01');
  });

  it('formats UTC days and reads today in UTC', () => {
    expect(isoDay(2026, 0, 1)).toBe('2026-01-01');
    expect(todayUtc(new Date('2026-10-23T23:30:00Z'))).toBe('2026-10-23');
  });
});
