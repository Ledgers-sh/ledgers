import { describe, expect, it } from 'vitest';
import type { Account, Unit } from '../api/types';
import {
  accountNeedsUnit,
  activeUnits,
  colourOf,
  DEFAULT_UNIT_COLOUR,
  hashWithUnit,
  routeOf,
  shareBps,
  unitFromHash,
  unitQuery,
} from './units';

const unit = (code: string, archived = false, colour = '#E9B44C'): Unit => ({
  code,
  name: `${code} unit`,
  colour,
  logo_document_id: null,
  archived,
  created_at: '2026-10-01T00:00:00Z',
});

describe('the unit scope in the location hash', () => {
  it('reads the code and upper-cases it', () => {
    expect(unitFromHash('overview?unit=ldg')).toBe('LDG');
    expect(unitFromHash('journal?period=2026-10&unit=FZ')).toBe('FZ');
  });

  it('is the whole company when absent, empty, or all', () => {
    expect(unitFromHash('overview')).toBeNull();
    expect(unitFromHash('overview?unit=')).toBeNull();
    expect(unitFromHash('overview?unit=all')).toBeNull();
    expect(unitFromHash('overview?unit=ALL')).toBeNull();
  });

  it('ignores a malformed code instead of sending it', () => {
    expect(unitFromHash('overview?unit=a')).toBeNull();
    expect(unitFromHash('overview?unit=bad%20code')).toBeNull();
    expect(unitFromHash('overview?unit=toolongcodeeeeeeeeeee')).toBeNull();
  });

  it('round-trips: setting the scope and reading it back gives the same code', () => {
    const hash = hashWithUnit('reports?period=2026-10', 'LDG');
    expect(hash).toBe('reports?period=2026-10&unit=LDG');
    expect(unitFromHash(hash)).toBe('LDG');
    expect(routeOf(hash)).toBe('reports');
  });

  it('removes the scope and keeps the route and the other parameters', () => {
    expect(hashWithUnit('reports?unit=LDG&period=2026-10', null)).toBe('reports?period=2026-10');
    expect(hashWithUnit('overview?unit=LDG', null)).toBe('overview');
    expect(hashWithUnit('accounts/6000', 'FZ')).toBe('accounts/6000?unit=FZ');
  });

  it('maps the scope to the API parameter, and omits it for the company', () => {
    expect(unitQuery('LDG')).toBe('LDG');
    expect(unitQuery(null)).toBeUndefined();
    expect(unitQuery(undefined)).toBeUndefined();
  });
});

describe('units in the app', () => {
  it('lists active units in code order and leaves archived ones out', () => {
    const list = activeUnits([unit('SQZ'), unit('FZ'), unit('OLD', true), unit('LDG')]);
    expect(list.map((u) => u.code)).toEqual(['FZ', 'LDG', 'SQZ']);
  });

  it('shows a valid colour and the default for anything else', () => {
    expect(colourOf(unit('FZ', false, '#123ABC'))).toBe('#123ABC');
    expect(colourOf({ colour: 'red' })).toBe(DEFAULT_UNIT_COLOUR);
    expect(colourOf(null)).toBe(DEFAULT_UNIT_COLOUR);
  });

  it('requires a unit on income, expense and inter-unit lines only', () => {
    const account = (account_type: string, tags: string[] = []): Pick<Account, 'account_type' | 'tags'> => ({ account_type, tags });
    expect(accountNeedsUnit(account('income'))).toBe(true);
    expect(accountNeedsUnit(account('expense'))).toBe(true);
    expect(accountNeedsUnit(account('asset'))).toBe(false);
    expect(accountNeedsUnit(account('liability', ['inter-unit-due-to']))).toBe(true);
    expect(accountNeedsUnit(account('equity', ['inter-unit-clearing']))).toBe(true);
    expect(accountNeedsUnit(undefined)).toBe(false);
  });

  it('computes a share in basis points without floats', () => {
    expect(shareBps(25n, 100n)).toBe(2500);
    expect(shareBps(1n, 3n)).toBe(3333);
    expect(shareBps(5n, 0n)).toBe(0);
  });
});
