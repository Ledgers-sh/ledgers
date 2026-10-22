import type { PostEntryInput } from '../api/types';
import type { CheckLine } from './balance';

export interface DraftLine {
  account: string;
  side: 'debit' | 'credit';
  amount: string;
  memo: string;
  /** The business unit the line belongs to; empty when none is chosen. */
  unit: string;
}

export interface DraftHeader {
  journalId: string;
  fiscalYear: number;
  date: string;
  description: string;
  currency: string;
  externalId: string;
}

/** The dimensions a line carries: its business unit, when one is chosen. */
function unitDimensions(unit: string): Record<string, string> {
  return unit === '' ? {} : { unit };
}

/** Converts parsed form lines into the body of POST /v1/entries. */
export function buildPostEntryInput(header: DraftHeader, lines: DraftLine[], minors: bigint[]): PostEntryInput {
  return {
    journal_id: header.journalId,
    fiscal_year: header.fiscalYear,
    entry: {
      journal_id: header.journalId,
      document_date: header.date,
      posting_date: header.date,
      tax_point_date: null,
      base_currency: header.currency,
      description: header.description,
      source_documents: [],
      external_id: header.externalId,
      source_app: 'ledgers-app',
      reverses: null,
      lines: lines.map((line, index) => ({
        account_number: line.account,
        side: line.side,
        base: { minor: minors[index].toString(), currency: header.currency },
        fx: null,
        tax_code: null,
        dimensions: unitDimensions(line.unit),
        memo: line.memo.trim() === '' ? null : line.memo.trim(),
      })),
    },
  };
}

export function toCheckLines(lines: DraftLine[], minors: (bigint | null)[]): CheckLine[] {
  return lines.map((line, index) => ({ side: line.side, minor: minors[index] ?? null }));
}
