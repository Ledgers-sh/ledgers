import { describe, expect, it } from 'vitest';
import type { Account } from '../api/types';
import { buildTransferInput, validateTransfer, type TransferDraft } from './transfer';

const account = (number: string, account_type: string): Account => ({
  number,
  name_key: number,
  display_name: null,
  account_type,
  normal_balance: account_type === 'income' || account_type === 'liability' ? 'credit' : 'debit',
  currency: null,
  parent: null,
  tags: [],
  is_postable: true,
  archived: false,
  balances: [],
});

const accounts = [account('4000', 'income'), account('6000', 'expense'), account('1000', 'asset')];

const valid: TransferDraft = {
  fromUnit: 'FZ',
  toUnit: 'LDG',
  amount: '1,250.00',
  recoveryAccount: '4000',
  costAccount: '6000',
  description: 'Shared hosting, Q3',
  documentDate: '2026-10-31',
  postingDate: '2026-10-31',
};

describe('the recharge form', () => {
  it('accepts a complete draft', () => {
    expect(validateTransfer(valid, 'SGD', accounts)).toEqual({});
  });

  it('needs two different units', () => {
    expect(validateTransfer({ ...valid, fromUnit: '' }, 'SGD', accounts).fromUnit).toBe('units.err_from');
    expect(validateTransfer({ ...valid, toUnit: '' }, 'SGD', accounts).toUnit).toBe('units.err_to');
    expect(validateTransfer({ ...valid, toUnit: 'FZ' }, 'SGD', accounts).toUnit).toBe('units.err_same');
  });

  it('needs a positive amount in the base currency', () => {
    expect(validateTransfer({ ...valid, amount: '' }, 'SGD', accounts).amount).toBe('units.err_amount');
    expect(validateTransfer({ ...valid, amount: '0' }, 'SGD', accounts).amount).toBe('units.err_amount');
    expect(validateTransfer({ ...valid, amount: '-5' }, 'SGD', accounts).amount).toBe('units.err_amount');
    expect(validateTransfer({ ...valid, amount: 'twelve' }, 'SGD', accounts).amount).toBe('units.err_amount_format');
  });

  it('takes an income account to recover and an expense account to book the cost', () => {
    expect(validateTransfer({ ...valid, recoveryAccount: '' }, 'SGD', accounts).recoveryAccount).toBe('units.err_recovery');
    expect(validateTransfer({ ...valid, recoveryAccount: '6000' }, 'SGD', accounts).recoveryAccount).toBe('units.err_recovery_type');
    expect(validateTransfer({ ...valid, costAccount: '' }, 'SGD', accounts).costAccount).toBe('units.err_cost');
    expect(validateTransfer({ ...valid, costAccount: '4000' }, 'SGD', accounts).costAccount).toBe('units.err_cost_type');
  });

  it('needs a description and well-formed dates', () => {
    expect(validateTransfer({ ...valid, description: '   ' }, 'SGD', accounts).description).toBe('units.err_description');
    expect(validateTransfer({ ...valid, postingDate: '31/10/2026' }, 'SGD', accounts).documentDate).toBe('units.err_date');
  });

  it('builds the request with the amount in minor units', () => {
    const input = buildTransferInput(valid, 'SGD');
    expect(input.amount).toEqual({ minor: '125000', currency: 'SGD' });
    expect(input.from_unit).toBe('FZ');
    expect(input.to_unit).toBe('LDG');
    expect(input.recovery_account).toBe('4000');
    expect(input.cost_account).toBe('6000');
  });
});
