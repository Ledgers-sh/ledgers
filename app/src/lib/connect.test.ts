import { describe, expect, it } from 'vitest';
import { costView, monthsBetween } from './connect';
import type { CostLine } from '../api/connect';

const line = (over: Partial<CostLine>): CostLine => ({
  period: '2026-07',
  unit_code: 'LDG',
  connector: 'cloudflare',
  connection_id: 'c1',
  currency: 'SGD',
  amount_minor: 1000,
  ...over,
});

describe('monthsBetween', () => {
  it('lists the months inclusive, across a year end', () => {
    expect(monthsBetween('2026-11', '2027-02')).toEqual(['2026-11', '2026-12', '2027-01', '2027-02']);
    expect(monthsBetween('2026-07', '2026-07')).toEqual(['2026-07']);
    expect(monthsBetween('2026-08', '2026-07')).toEqual([]);
  });
});

describe('costView', () => {
  const labels = {
    units: { LDG: { name: 'Ledgers', colour: '#7FA6C9' }, FZ: { name: 'Factory Zero', colour: '#3D5A80' } },
    sources: { cloudflare: 'Cloudflare', anthropic: 'Anthropic' },
  };

  it('stacks by unit per month and totals each row', () => {
    const view = costView({
      lines: [
        line({ unit_code: 'LDG', amount_minor: 600 }),
        line({ unit_code: 'FZ', amount_minor: 400 }),
        line({ period: '2026-08', unit_code: 'LDG', connector: 'anthropic', amount_minor: 250 }),
      ],
      periods: ['2026-07', '2026-08'],
      mode: 'unit',
      baseCurrency: 'SGD',
      unitLabels: labels.units,
      sourceLabels: labels.sources,
    });
    expect(view.series.map((s) => s.key)).toEqual(['FZ', 'LDG']);
    expect(view.series.find((s) => s.key === 'LDG')?.label).toBe('Ledgers');
    expect(view.rows[0]).toEqual({ period: '2026-07', values: { LDG: 600, FZ: 400 }, total: 1000 });
    expect(view.rows[1].values).toEqual({ LDG: 250 });
    expect(view.total).toBe(1250);
  });

  it('stacks by source when asked, and lists a source the books name', () => {
    const view = costView({
      lines: [line({ connector: 'cloudflare', amount_minor: 300 }), line({ connector: 'anthropic', amount_minor: 700 })],
      periods: ['2026-07'],
      mode: 'source',
      baseCurrency: 'SGD',
      unitLabels: labels.units,
      sourceLabels: labels.sources,
    });
    expect(view.series.map((s) => s.label)).toEqual(['Anthropic', 'Cloudflare']);
    expect(view.rows[0].total).toBe(1000);
  });

  it('never mixes another currency into the stacks; it is totalled apart', () => {
    const view = costView({
      lines: [line({ amount_minor: 500 }), line({ currency: 'USD', amount_minor: 4_150 }), line({ currency: 'USD', amount_minor: 50 })],
      periods: ['2026-07'],
      mode: 'unit',
      baseCurrency: 'SGD',
      unitLabels: labels.units,
      sourceLabels: labels.sources,
    });
    expect(view.total).toBe(500);
    expect(view.otherCurrencies).toEqual({ USD: 4200 });
  });

  it('keeps every month of the range even when it has no cost', () => {
    const view = costView({
      lines: [],
      periods: ['2026-01', '2026-02'],
      mode: 'unit',
      baseCurrency: 'SGD',
      unitLabels: {},
      sourceLabels: {},
    });
    expect(view.rows.map((row) => row.total)).toEqual([0, 0]);
    expect(view.series).toEqual([]);
  });
});
