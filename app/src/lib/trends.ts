/**
 * The trend maths behind the Overview charts. Money stays BigInt minor units
 * end to end; the only floats are display ratios (percent change, runway
 * months), each rounded to one decimal here so the screen and the tests agree.
 */
import type { TrendMonth } from '../api/types';

/** A month range, both ends inclusive, as "YYYY-MM". */
export interface MonthRange {
  from: string;
  to: string;
}

export type PresetId = 'thisMonth' | 'lastMonth' | 'thisQuarter' | 'thisYear' | 'last12' | 'custom';

const PRESET_IDS: readonly PresetId[] = ['thisMonth', 'lastMonth', 'thisQuarter', 'thisYear', 'last12', 'custom'];

export function isPresetId(value: string): value is PresetId {
  return (PRESET_IDS as readonly string[]).includes(value);
}

/** The "YYYY-MM" key of a month index (year * 12 + month - 1). */
function keyOf(index: number): string {
  const year = Math.floor(index / 12);
  const month = (index % 12) + 1;
  return `${year}-${String(month).padStart(2, '0')}`;
}

/** The month index of a "YYYY-MM" key. */
function indexOf(period: string): number {
  return Number(period.slice(0, 4)) * 12 + Number(period.slice(5, 7)) - 1;
}

export function monthIndexOf(date: Date): number {
  return date.getFullYear() * 12 + date.getMonth();
}

/** Every month of the range, in order. */
export function monthsIn(range: MonthRange): string[] {
  const start = indexOf(range.from);
  const end = indexOf(range.to);
  const out: string[] = [];
  for (let index = start; index <= end; index += 1) out.push(keyOf(index));
  return out;
}

/** How many months the range covers (at least one). */
export function monthCount(range: MonthRange): number {
  return Math.max(1, indexOf(range.to) - indexOf(range.from) + 1);
}

/** The range of the same length immediately before `range`. */
export function previousRange(range: MonthRange): MonthRange {
  const length = monthCount(range);
  const end = indexOf(range.from) - 1;
  return { from: keyOf(end - length + 1), to: keyOf(end) };
}

/** The twelve months ending at `to`. */
export function rollingYear(to: string): MonthRange {
  return { from: keyOf(indexOf(to) - 11), to };
}

/** The range a preset means on `today`. Quarter and year run to date. */
export function presetRange(preset: Exclude<PresetId, 'custom'>, today: Date): MonthRange {
  const current = monthIndexOf(today);
  const quarterStart = current - (today.getMonth() % 3);
  switch (preset) {
    case 'thisMonth':
      return { from: keyOf(current), to: keyOf(current) };
    case 'lastMonth':
      return { from: keyOf(current - 1), to: keyOf(current - 1) };
    case 'thisQuarter':
      return { from: keyOf(quarterStart), to: keyOf(current) };
    case 'thisYear':
      return { from: keyOf(Math.floor(current / 12) * 12), to: keyOf(current) };
    case 'last12':
      return { from: keyOf(current - 11), to: keyOf(current) };
  }
}

/** The preset that matches `range` on `today`, or 'custom'. */
export function presetOf(range: MonthRange, today: Date): PresetId {
  const presets: Exclude<PresetId, 'custom'>[] = ['thisMonth', 'lastMonth', 'thisQuarter', 'thisYear', 'last12'];
  const match = presets.find((preset) => {
    const candidate = presetRange(preset, today);
    return candidate.from === range.from && candidate.to === range.to;
  });
  return match ?? 'custom';
}

/** Sorts and clamps a range so `from` is never after `to`. */
export function normaliseRange(range: MonthRange): MonthRange {
  return range.from <= range.to ? range : { from: range.to, to: range.from };
}

export interface Change {
  /** `up` or `down` against the previous period, `flat` when equal, `new` when the previous period had nothing to compare. */
  direction: 'up' | 'down' | 'flat' | 'new';
  /** Size of the change in percent, one decimal, unsigned. Null for `flat` and `new`. */
  percent: number | null;
}

/**
 * Change of `current` against `previous`, as a percentage of the absolute
 * previous value. A previous value of zero has no percentage: it is `new`
 * when `current` is not zero and `flat` when both are zero.
 */
export function changeOf(current: bigint, previous: bigint): Change {
  if (current === previous) return { direction: 'flat', percent: null };
  if (previous === 0n) return { direction: 'new', percent: null };
  const delta = current - previous;
  const magnitude = previous < 0n ? -previous : previous;
  const direction = delta > 0n ? 'up' : 'down';
  const tenths = (absolute(delta) * 1000n) / magnitude;
  return { direction, percent: Number(tenths) / 10 };
}

function absolute(value: bigint): bigint {
  return value < 0n ? -value : value;
}

/** Sum of one money field across months, in minor units. */
export function sumOf(months: readonly TrendMonth[], field: 'revenue' | 'expenses' | 'net_result'): bigint {
  return months.reduce((total, month) => total + BigInt(month[field].minor), 0n);
}

/** The value of a balance field at the end of the range (the last month). Zero for an empty range. */
export function endBalance(months: readonly TrendMonth[], field: 'cash' | 'receivables' | 'payables'): bigint {
  const last = months[months.length - 1];
  return last ? BigInt(last[field].minor) : 0n;
}

/** Average monthly expenses over `months` in minor units, rounded down. Zero for no months. */
export function averageMonthlyExpenses(months: readonly TrendMonth[]): bigint {
  if (months.length === 0) return 0n;
  return sumOf(months, 'expenses') / BigInt(months.length);
}

/**
 * How many months the cash covers at the average monthly expenses, to one
 * decimal. Null when there are no expenses to cover (the figure would be
 * infinite); zero when the cash is not positive.
 */
export function runwayMonths(cash: bigint, averageExpenses: bigint): number | null {
  if (averageExpenses <= 0n) return null;
  if (cash <= 0n) return 0;
  return Number((cash * 10n) / averageExpenses) / 10;
}

/** A readable runway: whole months from ten up, one decimal below that. */
export function formatRunway(months: number): string {
  return months >= 10 ? String(Math.floor(months)) : months.toFixed(1);
}
