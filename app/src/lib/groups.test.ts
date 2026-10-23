import { describe, expect, it } from 'vitest';
import type { GroupView, PairView, PeriodDocView } from '../api/groups';
import {
  accountsFor,
  accrualPreview,
  canChangeSettings,
  canMoveMoney,
  canPostPeriods,
  expectedWhtBp,
  formProblem,
  formatBpPercent,
  groupMembershipBlocker,
  groupNavVisible,
  monthEnd,
  nextAction,
  openMonth,
  pairHasDifference,
  parsePercentToBp,
  periodsBetween,
  settlementPreview,
  timelineCells,
  whtDisplay,
  type SideAccounts,
} from './groups';
import { countryNote, packRoyaltyWhtBp } from './countryNotes';

function doc(overrides: Partial<PeriodDocView>): PeriodDocView {
  return {
    id: 'IC-1-2026-01',
    side: 'licensor',
    period: '2026-01',
    status: 'draft',
    currency: 'EUR',
    basis_minor: 500000,
    gross_minor: 250000,
    wht_minor: 25000,
    net_minor: 225000,
    accrual_entry_id: null,
    settle_entry_id: null,
    last_error: null,
    ...overrides,
  };
}

describe('roles', () => {
  it('lets owners and admins change settings, accountants and owners post, and only owners and admins move money', () => {
    expect(canChangeSettings('owner')).toBe(true);
    expect(canChangeSettings('admin')).toBe(true);
    expect(canChangeSettings('accountant')).toBe(false);
    expect(canPostPeriods('accountant')).toBe(true);
    expect(canPostPeriods('viewer')).toBe(false);
    expect(canMoveMoney('accountant')).toBe(false);
    expect(canMoveMoney('admin')).toBe(true);
  });

  it('refuses a group unless the person owns every member, and says which company is the problem', () => {
    const companies = [
      { id: 'A', name: 'Alpha', role: 'owner' },
      { id: 'B', name: 'Beta', role: 'admin' },
    ];
    expect(groupMembershipBlocker(['A'], companies)).toBe('fewer_than_two');
    expect(groupMembershipBlocker(['A', 'B'], companies)).toBe('not_owner');
    expect(groupMembershipBlocker(['A', 'C'], companies)).toBe('unknown_company');
    expect(groupMembershipBlocker(['A', 'A'], [{ id: 'A', name: 'Alpha', role: 'owner' }])).toBe(null);
  });

  it('shows the Group nav to a member, or to someone who owns two companies, and to no one else', () => {
    const owned = [
      { id: 'A', name: 'Alpha', role: 'owner' },
      { id: 'B', name: 'Beta', role: 'owner' },
    ];
    const group = { id: 'G' } as GroupView;
    expect(groupNavVisible([group], [])).toBe(true);
    expect(groupNavVisible([], owned)).toBe(true);
    expect(groupNavVisible([], [owned[0]!, { id: 'C', name: 'C', role: 'viewer' }])).toBe(false);
  });
});

describe('ownership', () => {
  it('parses a percentage the person typed into basis points', () => {
    expect(parsePercentToBp('100')).toBe(10000);
    expect(parsePercentToBp('62.5')).toBe(6250);
    expect(parsePercentToBp('62,25')).toBe(6225);
    expect(parsePercentToBp('0')).toBe(null);
    expect(parsePercentToBp('100.01')).toBe(null);
    expect(parsePercentToBp('12.345')).toBe(null);
    expect(parsePercentToBp('abc')).toBe(null);
  });

  it('formats basis points with the locale separator', () => {
    expect(formatBpPercent(6250, 'en-US')).toBe('62.5');
    expect(formatBpPercent(6250, 'de-DE')).toBe('62,5');
    expect(formatBpPercent(10000, 'en-US')).toBe('100');
  });
});

describe('periods', () => {
  it('lists the months from one to another, inclusive, and nothing when the range is backwards', () => {
    expect(periodsBetween('2026-11', '2027-02')).toEqual(['2026-11', '2026-12', '2027-01', '2027-02']);
    expect(periodsBetween('2026-03', '2026-03')).toEqual(['2026-03']);
    expect(periodsBetween('2026-05', '2026-03')).toEqual([]);
    expect(periodsBetween('bad', '2026-03')).toEqual([]);
  });

  it('gives the last day of a month for the balance sheet date', () => {
    expect(monthEnd('2026-02')).toBe('2026-02-28');
    expect(monthEnd('2028-02')).toBe('2028-02-29');
    expect(monthEnd('2026-12')).toBe('2026-12-31');
  });
});

describe('withholding display', () => {
  it('names the source of each withholding rate in words', () => {
    expect(whtDisplay({ wht_bp: 1000, wht_basis: 'pack', treaty_override_bp: null }).key).toBe('groups.wht.pack');
    expect(whtDisplay({ wht_bp: 500, wht_basis: 'treaty_override', treaty_override_bp: 500 }).key).toBe('groups.wht.treaty_override');
    expect(whtDisplay({ wht_bp: 0, wht_basis: 'resident', treaty_override_bp: null }).key).toBe('groups.wht.resident');
    expect(whtDisplay({ wht_bp: 0, wht_basis: 'no_rate_on_file', treaty_override_bp: null }).key).toBe('groups.wht.no_rate_on_file');
    expect(whtDisplay({ wht_bp: 0, wht_basis: 'not_applicable', treaty_override_bp: null }).key).toBe('groups.wht.not_applicable');
  });

  it('expects the treaty override first, then the payer pack rate, and no withholding for a resident payee', () => {
    expect(expectedWhtBp({ licenseeCountry: 'SG', licensorCountry: 'NL', packRateBp: 1000, treatyOverrideBp: 500 })).toEqual({
      bp: 500,
      basis: 'treaty_override',
    });
    expect(expectedWhtBp({ licenseeCountry: 'SG', licensorCountry: 'NL', packRateBp: 1000, treatyOverrideBp: null })).toEqual({
      bp: 1000,
      basis: 'pack',
    });
    expect(expectedWhtBp({ licenseeCountry: 'SG', licensorCountry: 'SG', packRateBp: 1000, treatyOverrideBp: 500 })).toEqual({
      bp: 0,
      basis: 'resident',
    });
    expect(expectedWhtBp({ licenseeCountry: 'NL', licensorCountry: 'DE', packRateBp: null, treatyOverrideBp: null })).toEqual({
      bp: 0,
      basis: 'no_rate_on_file',
    });
  });

  it('reads the rates the country packs carry, and nothing for the Netherlands', () => {
    expect(packRoyaltyWhtBp('sg')).toBe(1000);
    expect(packRoyaltyWhtBp('ID')).toBe(2000);
    expect(packRoyaltyWhtBp('NL')).toBe(null);
    expect(packRoyaltyWhtBp('DE')).toBe(null);
    expect(countryNote('NL')?.notes[0]?.topic).toBe('innovatiebox');
    expect(countryNote('SG')?.whtSource?.url).toMatch(/^https:\/\/www\.iras\.gov\.sg\//);
    expect(countryNote('ID')?.whtSource?.url).toBe('https://pajak.go.id/en/node/34286');
  });
});

describe('preview of the entries', () => {
  it('shows the licensor invoice and the licensee bill with the withholding split, both balanced', () => {
    const licensor = doc({ side: 'licensor' });
    const licensee = doc({ side: 'licensee', gross_minor: 250000, wht_minor: 25000, net_minor: 225000 });
    const preview = accrualPreview(licensor, licensee, '2026-01');
    expect(preview.licensor.lines).toEqual([
      { role: 'ic_receivable', side: 'debit', minor: 250000 },
      { role: 'royalty_income', side: 'credit', minor: 250000 },
    ]);
    expect(preview.licensee.lines).toEqual([
      { role: 'royalty_expense', side: 'debit', minor: 250000 },
      { role: 'ic_payable', side: 'credit', minor: 225000 },
      { role: 'wht_payable', side: 'credit', minor: 25000 },
    ]);
    expect(preview.licensor.balanced).toBe(true);
    expect(preview.licensee.balanced).toBe(true);
    expect(preview.licensee.headline).toBe(225000);
    expect(preview.licensor.headline).toBe(250000);
  });

  it('drops a zero withholding line and still balances', () => {
    const licensee = doc({ side: 'licensee', wht_minor: 0, net_minor: 250000 });
    const preview = accrualPreview(undefined, licensee, '2026-01');
    expect(preview.licensee.lines.map((line) => line.role)).toEqual(['royalty_expense', 'ic_payable']);
    expect(preview.licensee.balanced).toBe(true);
  });

  it('settles with the withholding held by the licensor as a receivable, both sides balanced', () => {
    const licensor = doc({ side: 'licensor', status: 'posted' });
    const licensee = doc({ side: 'licensee', status: 'posted' });
    const preview = settlementPreview(licensor, licensee, '2026-01');
    expect(preview.licensor.lines).toEqual([
      { role: 'bank', side: 'debit', minor: 225000 },
      { role: 'wht_receivable', side: 'debit', minor: 25000 },
      { role: 'ic_receivable', side: 'credit', minor: 250000 },
    ]);
    expect(preview.licensee.lines).toEqual([
      { role: 'ic_payable', side: 'debit', minor: 225000 },
      { role: 'bank', side: 'credit', minor: 225000 },
    ]);
    expect(preview.licensor.balanced && preview.licensee.balanced).toBe(true);
  });
});

describe('timeline', () => {
  it('reads a month across both sides: a failure fails the month, both settled settles it', () => {
    const cells = timelineCells([
      doc({ period: '2026-02', side: 'licensor', status: 'posted' }),
      doc({ period: '2026-02', side: 'licensee', status: 'failed', last_error: 'account 1000 is not postable' }),
      doc({ period: '2026-01', side: 'licensor', status: 'settled' }),
      doc({ period: '2026-01', side: 'licensee', status: 'settled' }),
      doc({ period: '2026-03', side: 'licensor', status: 'draft' }),
      doc({ period: '2026-03', side: 'licensee', status: 'draft' }),
    ]);
    expect(cells.map((cell) => [cell.period, cell.state])).toEqual([
      ['2026-01', 'settled'],
      ['2026-02', 'failed'],
      ['2026-03', 'draft'],
    ]);
    expect(nextAction(cells[1]!)).toBe('retry');
    expect(nextAction(cells[2]!)).toBe('post');
    expect(nextAction(cells[0]!)).toBe('none');
  });

  it('offers settle for a posted month only', () => {
    const cells = timelineCells([
      doc({ period: '2026-04', side: 'licensor', status: 'posted' }),
      doc({ period: '2026-04', side: 'licensee', status: 'posted' }),
    ]);
    expect(cells[0]?.state).toBe('posted');
    expect(nextAction(cells[0]!)).toBe('settle');
  });
});

describe('reconciliation', () => {
  const pair = (overrides: Partial<PairView>): PairView =>
    ({
      agreement_id: 'A',
      group_id: 'G',
      licensor: { id: 'L', name: 'IP' },
      licensee: { id: 'E', name: 'Ops' },
      currency: 'EUR',
      receivable_minor: 0,
      payable_minor: 0,
      balance_difference_minor: 0,
      income_minor: 0,
      expense_minor: 0,
      income_difference_minor: 0,
      status: 'balanced',
      open_periods: [],
      periods: [],
      ...overrides,
    }) as PairView;

  it('finds the open month and any difference', () => {
    const open = pair({ open_periods: ['2026-01', '2026-02'], balance_difference_minor: 250000 });
    expect(openMonth(open)).toBe('2026-02');
    expect(pairHasDifference(open)).toBe(true);
    expect(openMonth(pair({}))).toBe(null);
    expect(pairHasDifference(pair({ income_difference_minor: 5 }))).toBe(true);
    expect(pairHasDifference(pair({}))).toBe(false);
  });
});

describe('account pickers', () => {
  const chart = [
    { number: '4060', account_type: 'income', is_postable: true, archived: false },
    { number: '6100', account_type: 'expense', is_postable: true, archived: false },
    { number: '1200', account_type: 'asset', is_postable: true, archived: false },
    { number: '1210', account_type: 'asset', is_postable: false, archived: false },
    { number: '2060', account_type: 'liability', is_postable: true, archived: true },
    { number: '2070', account_type: 'liability', is_postable: true, archived: false },
  ];

  it('offers the right types for each side and role, live and postable only', () => {
    expect(accountsFor(chart, 'royalty', 'licensor').map((a) => a.number)).toEqual(['4060']);
    expect(accountsFor(chart, 'royalty', 'licensee').map((a) => a.number)).toEqual(['6100']);
    expect(accountsFor(chart, 'ic', 'licensor').map((a) => a.number)).toEqual(['1200']);
    expect(accountsFor(chart, 'ic', 'licensee').map((a) => a.number)).toEqual(['2070']);
    expect(accountsFor(chart, 'bank', 'licensee').map((a) => a.number)).toEqual(['1200']);
  });
});

describe('form checks before the server', () => {
  const accounts: SideAccounts = { royalty: '4060', ic: '1200', wht: '', bank: '1000', unit: '' };
  const base = {
    groupId: 'G',
    licensorId: 'L',
    licenseeId: 'E',
    licensorRole: 'owner',
    licenseeRole: 'admin',
    licensorCurrency: 'EUR',
    licenseeCurrency: 'EUR',
    kind: 'royalty' as const,
    basis: 'pct_of_revenue_bp' as const,
    ratePercent: '5',
    fixedText: '',
    currency: 'EUR',
    treatyPercent: '',
    licensorAccounts: accounts,
    licenseeAccounts: { royalty: '6100', ic: '2070', wht: '2060', bank: '1000', unit: '' },
    licensorUnitsRequired: false,
    licenseeUnitsRequired: false,
  };

  it('passes a complete form and names the first problem otherwise', () => {
    expect(formProblem(base)).toBe(null);
    expect(formProblem({ ...base, licensorId: 'E' })).toBe('agreements.err.same_company');
    expect(formProblem({ ...base, licenseeRole: 'viewer' })).toBe('agreements.err.settings_both');
    expect(formProblem({ ...base, licenseeCurrency: 'SGD' })).toBe('agreements.err.currency_mismatch');
    expect(formProblem({ ...base, ratePercent: '150' })).toBe('agreements.err.rate');
    expect(formProblem({ ...base, treatyPercent: '-1' })).toBe('agreements.err.treaty');
    expect(formProblem({ ...base, licensorUnitsRequired: true })).toBe('agreements.err.unit');
  });

  it('wants a positive fixed amount for a fixed monthly fee', () => {
    expect(formProblem({ ...base, basis: 'fixed_monthly', fixedText: '0' })).toBe('agreements.err.fixed');
    expect(formProblem({ ...base, basis: 'fixed_monthly', fixedText: '1200,50' })).toBe(null);
  });
});
