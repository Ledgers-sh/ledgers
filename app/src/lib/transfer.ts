/**
 * The recharge between two business units (entries.transfer): the form's
 * checks and the request it builds. A recharge recovers a shared cost from the
 * unit that paid it: income on the paying unit, expense on the receiving unit,
 * both in the company's base currency.
 */
import type { Account, EntriesTransferInput } from '../api/types';
import { parseMajorToMinor } from './money';

export interface TransferDraft {
  fromUnit: string;
  toUnit: string;
  amount: string;
  recoveryAccount: string;
  costAccount: string;
  description: string;
  documentDate: string;
  postingDate: string;
}

export type TransferField = keyof TransferDraft;

/** Message keys for each problem; the dialog translates them. */
export type TransferProblem =
  | 'units.err_from'
  | 'units.err_same'
  | 'units.err_to'
  | 'units.err_amount_format'
  | 'units.err_amount'
  | 'units.err_recovery'
  | 'units.err_recovery_type'
  | 'units.err_cost'
  | 'units.err_cost_type'
  | 'units.err_description'
  | 'units.err_date';

const DATE = /^\d{4}-\d{2}-\d{2}$/;

/** The problems with a recharge draft, by field. An empty result means it can be sent. */
export function validateTransfer(
  draft: TransferDraft,
  currency: string,
  accounts: readonly Account[],
): Partial<Record<TransferField, TransferProblem>> {
  const problems: Partial<Record<TransferField, TransferProblem>> = {};
  if (draft.fromUnit === '') problems.fromUnit = 'units.err_from';
  if (draft.toUnit === '') problems.toUnit = 'units.err_to';
  if (draft.fromUnit !== '' && draft.fromUnit === draft.toUnit) problems.toUnit = 'units.err_same';
  const amount = draft.amount.trim() === '' ? null : parseMajorToMinor(draft.amount, currency);
  if (draft.amount.trim().startsWith('-')) problems.amount = 'units.err_amount';
  else if (draft.amount.trim() !== '' && amount === null) problems.amount = 'units.err_amount_format';
  else if (amount === null || amount <= 0n) problems.amount = 'units.err_amount';
  const recovery = accounts.find((account) => account.number === draft.recoveryAccount);
  if (draft.recoveryAccount === '') problems.recoveryAccount = 'units.err_recovery';
  else if (recovery && recovery.account_type !== 'income') problems.recoveryAccount = 'units.err_recovery_type';
  const cost = accounts.find((account) => account.number === draft.costAccount);
  if (draft.costAccount === '') problems.costAccount = 'units.err_cost';
  else if (cost && cost.account_type !== 'expense') problems.costAccount = 'units.err_cost_type';
  if (draft.description.trim() === '') problems.description = 'units.err_description';
  if (!DATE.test(draft.documentDate) || !DATE.test(draft.postingDate)) problems.documentDate = 'units.err_date';
  return problems;
}

/** The request body of entries.transfer for a valid draft. */
export function buildTransferInput(draft: TransferDraft, currency: string): EntriesTransferInput {
  const minor = parseMajorToMinor(draft.amount, currency) ?? 0n;
  return {
    from_unit: draft.fromUnit,
    to_unit: draft.toUnit,
    amount: { minor: minor.toString(), currency },
    document_date: draft.documentDate,
    posting_date: draft.postingDate,
    tax_point_date: null,
    description: draft.description.trim(),
    recovery_account: draft.recoveryAccount,
    cost_account: draft.costAccount,
  };
}
