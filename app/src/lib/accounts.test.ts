import { describe, expect, it } from 'vitest';
import type { Account } from '../api/types';
import {
  ACCOUNT_TYPES,
  accountLabel,
  buildChart,
  filterChart,
  hasErrors,
  hasPostings,
  parentOptions,
  parseTags,
  tagErrors,
  validateDraft,
  type DraftInput,
} from './accounts';

function account(overrides: Partial<Account> & Pick<Account, 'number'>): Account {
  return {
    name_key: `account.test.${overrides.number}`,
    display_name: null,
    account_type: 'asset',
    normal_balance: 'debit',
    currency: null,
    parent: null,
    tags: [],
    is_postable: true,
    archived: false,
    balances: [],
    ...overrides,
  };
}

const label = (a: Account) => accountLabel(a, (key) => `T(${key})`);

const CHART: Account[] = [
  account({ number: '1000', account_type: 'asset', is_postable: false, display_name: 'Current assets' }),
  account({ number: '1010', account_type: 'asset', parent: '1000', display_name: 'Cash', tags: ['cash'], balances: [{ minor: '500', currency: 'EUR' }] }),
  account({ number: '1020', account_type: 'asset', parent: '1000', display_name: 'Bank', tags: ['bank'] }),
  account({ number: '1500', account_type: 'asset', display_name: 'Fixed assets' }),
  account({ number: '1510', account_type: 'asset', parent: '1500', display_name: 'Office equipment', archived: true }),
  account({ number: '2000', account_type: 'liability', display_name: 'Payables' }),
  account({ number: '4000', account_type: 'income', display_name: 'Sales' }),
  account({ number: '6000', account_type: 'expense', display_name: 'Rent' }),
  // A child whose parent is of another class: shown as a root, not dropped.
  account({ number: '6900', account_type: 'expense', parent: '1000', display_name: 'Misplaced' }),
];

describe('accountLabel', () => {
  it('prefers the custom name and falls back to the translated template name', () => {
    expect(label(account({ number: '1', name_key: 'account.x.cash', display_name: 'Till' }))).toBe('Till');
    expect(label(account({ number: '1', name_key: 'account.x.cash', display_name: null }))).toBe('T(account.x.cash)');
    expect(label(account({ number: '1', name_key: 'account.x.cash', display_name: '   ' }))).toBe('T(account.x.cash)');
  });
});

describe('buildChart', () => {
  const groups = buildChart(CHART);

  it('groups by class in the fixed order, with every class present', () => {
    expect(groups.map((g) => g.type)).toEqual([...ACCOUNT_TYPES]);
    expect(groups.find((g) => g.type === 'equity')?.rows).toEqual([]);
  });

  it('nests children under their parent, ordered by number, with depth', () => {
    const assets = groups[0].rows.map((row) => [row.account.number, row.depth]);
    expect(assets).toEqual([
      ['1000', 0],
      ['1010', 1],
      ['1020', 1],
      ['1500', 0],
      ['1510', 1],
    ]);
  });

  it('marks headings and keeps archived children in the tree', () => {
    const rows = groups[0].rows;
    expect(rows.find((r) => r.account.number === '1000')?.hasChildren).toBe(true);
    expect(rows.find((r) => r.account.number === '1010')?.hasChildren).toBe(false);
    expect(rows.find((r) => r.account.number === '1510')?.account.archived).toBe(true);
  });

  it('shows a child of another class as a root of its own class', () => {
    const expense = groups.find((g) => g.type === 'expense')?.rows.map((r) => [r.account.number, r.depth]);
    expect(expense).toEqual([
      ['6000', 0],
      ['6900', 0],
    ]);
  });

  it('survives a parent cycle without looping', () => {
    const cyclic = buildChart([
      account({ number: '1100', parent: '1200' }),
      account({ number: '1200', parent: '1100' }),
    ]);
    expect(cyclic[0].rows).toHaveLength(0);
  });
});

describe('filterChart', () => {
  const groups = buildChart(CHART);

  it('hides archived accounts by default and shows them on request', () => {
    const active = filterChart(groups, { query: '', status: 'active', label }).flatMap((g) => g.rows.map((r) => r.account.number));
    expect(active).not.toContain('1510');
    const archived = filterChart(groups, { query: '', status: 'archived', label }).flatMap((g) => g.rows.map((r) => r.account.number));
    // The archived child keeps its heading for context.
    expect(archived).toEqual(['1500', '1510']);
  });

  it('keeps the ancestors of a search match so it stays in place', () => {
    const found = filterChart(groups, { query: 'bank', status: 'all', label }).flatMap((g) => g.rows.map((r) => r.account.number));
    expect(found).toEqual(['1000', '1020']);
  });

  it('searches the number, the label and the tags, ignoring case', () => {
    const byNumber = filterChart(groups, { query: ' 4000 ', status: 'all', label }).flatMap((g) => g.rows.map((r) => r.account.number));
    expect(byNumber).toEqual(['4000']);
    const byTag = filterChart(groups, { query: 'CASH', status: 'all', label }).flatMap((g) => g.rows.map((r) => r.account.number));
    expect(byTag).toEqual(['1000', '1010']);
  });

  it('drops classes with nothing left', () => {
    const none = filterChart(groups, { query: 'zzz', status: 'all', label });
    expect(none).toEqual([]);
  });
});

describe('validateDraft', () => {
  const blank: DraftInput = { number: '', name: '', type: '', parent: '', tags: '', currency: '' };

  it('requires a number, a name and a class', () => {
    expect(validateDraft(blank, CHART)).toEqual({ number: 'required', name: 'required', type: 'required' });
  });

  it('checks the number format and uniqueness against the whole chart, archived included', () => {
    const base = { ...blank, name: 'X', type: 'asset' };
    expect(validateDraft({ ...base, number: '1' }, CHART).number).toBe('format');
    expect(validateDraft({ ...base, number: '12345678901' }, CHART).number).toBe('format');
    expect(validateDraft({ ...base, number: '12a4' }, CHART).number).toBe('format');
    expect(validateDraft({ ...base, number: '0120' }, CHART).number).toBeUndefined();
    expect(validateDraft({ ...base, number: '1510' }, CHART).number).toBe('taken');
  });

  it('keeps the parent in the same class and without postings', () => {
    const base = { ...blank, number: '1600', name: 'X' };
    expect(validateDraft({ ...base, type: 'liability', parent: '1000' }, CHART).parent).toBe('same_type');
    expect(validateDraft({ ...base, type: 'asset', parent: '1010' }, CHART).parent).toBe('has_postings');
    expect(validateDraft({ ...base, type: 'asset', parent: '1500' }, CHART).parent).toBeUndefined();
  });

  it('validates tags and currency', () => {
    const base = { ...blank, number: '1600', name: 'X', type: 'asset' };
    expect(validateDraft({ ...base, tags: 'Bad_Tag' }, CHART).tags).toBe('format');
    expect(validateDraft({ ...base, tags: 'bank, bank' }, CHART).tags).toBe('duplicate');
    expect(validateDraft({ ...base, tags: 'bank vat-input' }, CHART).tags).toBeUndefined();
    expect(validateDraft({ ...base, currency: 'eur' }, CHART).currency).toBe('format');
    expect(validateDraft({ ...base, currency: 'EUR' }, CHART).currency).toBeUndefined();
    expect(hasErrors(validateDraft({ ...base, tags: 'bank' }, CHART))).toBe(false);
  });

  it('limits the name length', () => {
    const base = { ...blank, number: '1600', type: 'asset' };
    expect(validateDraft({ ...base, name: 'x'.repeat(201) }, CHART).name).toBe('too_long');
  });
});

describe('helpers', () => {
  it('parses tags on commas and spaces', () => {
    expect(parseTags(' bank,  vat-input ,cash ')).toEqual(['bank', 'vat-input', 'cash']);
    expect(parseTags('')).toEqual([]);
  });

  it('reports postings and the tag errors of an edit form', () => {
    expect(hasPostings(CHART[1])).toBe(true);
    expect(hasPostings(CHART[2])).toBe(false);
    expect(tagErrors('bank')).toBeUndefined();
    expect(tagErrors('Bank')).toBe('format');
  });

  it('offers only same-class, active, unposted parents', () => {
    expect(parentOptions(CHART, 'asset').map((a) => a.number)).toEqual(['1000', '1020', '1500']);
  });
});
