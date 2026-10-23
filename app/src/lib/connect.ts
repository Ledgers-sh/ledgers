/**
 * Connected-source costs as chart data (issue #142): one row per month, one
 * series per business unit or per source. Only lines in the company's base
 * currency are charted, so the stacks add up; other currencies are totalled
 * apart and named, never converted.
 */
import type { CostLine } from '../api/connect';

/** The months from `from` to `to` (both `YYYY-MM`), inclusive, in order. */
export function monthsBetween(from: string, to: string): string[] {
  const out: string[] = [];
  let [year, month] = from.split('-').map(Number);
  const [endYear, endMonth] = to.split('-').map(Number);
  while (year < endYear || (year === endYear && month <= endMonth)) {
    out.push(`${year}-${String(month).padStart(2, '0')}`);
    month += 1;
    if (month > 12) {
      month = 1;
      year += 1;
    }
  }
  return out;
}

export interface CostSeries {
  key: string;
  label: string;
  colour: string;
}

export interface CostRow {
  period: string;
  values: Record<string, number>;
  total: number;
}

export interface CostView {
  rows: CostRow[];
  series: CostSeries[];
  /** Minor units in the base currency, all rows and series. */
  total: number;
  /** Minor units in other currencies, not charted, by currency code. */
  otherCurrencies: Record<string, number>;
}

/** Colours for units the company has not given one, in order. Distinct from the agent and person accents. */
const FALLBACK_COLOURS = ['#E9B44C', '#8DB89E', '#C98B7E', '#A9B6C6', '#B7A3D6', '#D98C5F', '#9AB973'];

/** The fixed palette for sources; units use their own colours. */
export const SOURCE_COLOURS: Record<string, string> = {
  cloudflare: '#E9B44C',
  anthropic: '#7FA6C9',
  planetscale: '#9AB973',
};

/**
 * Builds the chart rows for a period range. `mode` picks the series: the units
 * (`unit`) or the sources (`source`). Units with no colour get a neutral one.
 */
export function costView(input: {
  lines: CostLine[];
  periods: string[];
  mode: 'unit' | 'source';
  baseCurrency: string;
  unitLabels: Record<string, { name: string; colour: string }>;
  sourceLabels: Record<string, string>;
}): CostView {
  const { lines, periods, mode, baseCurrency, unitLabels, sourceLabels } = input;
  const otherCurrencies: Record<string, number> = {};
  const keys = new Set<string>();
  const byPeriod = new Map<string, Record<string, number>>(periods.map((period) => [period, {}]));
  let total = 0;
  for (const line of lines) {
    if (line.currency !== baseCurrency) {
      otherCurrencies[line.currency] = (otherCurrencies[line.currency] ?? 0) + line.amount_minor;
      continue;
    }
    const key = mode === 'unit' ? line.unit_code : line.connector;
    const row = byPeriod.get(line.period);
    if (!row) continue;
    row[key] = (row[key] ?? 0) + line.amount_minor;
    keys.add(key);
    total += line.amount_minor;
  }
  let fallback = 0;
  const series: CostSeries[] = [...keys].sort().map((key) => {
    if (mode === 'source') {
      return { key, label: sourceLabels[key] ?? key, colour: SOURCE_COLOURS[key] ?? '#8C96A8' };
    }
    const colour = unitLabels[key]?.colour ?? FALLBACK_COLOURS[fallback++ % FALLBACK_COLOURS.length];
    return { key, label: unitLabels[key]?.name ?? key, colour };
  });
  const rows: CostRow[] = periods.map((period) => {
    const values = byPeriod.get(period) ?? {};
    const sum = Object.values(values).reduce((acc, value) => acc + value, 0);
    return { period, values, total: sum };
  });
  return { rows, series, total, otherCurrencies };
}
