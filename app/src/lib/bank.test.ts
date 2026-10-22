import { describe, expect, it } from 'vitest';
import {
  accountDraftProblems,
  conditionParts,
  detectFormat,
  distinctiveWord,
  EMPTY_FILTERS,
  filtersToQuery,
  formatIban,
  formatMinorPlain,
  guessMapping,
  ibanProblem,
  journalPreview,
  lineStatus,
  mappingProblem,
  parseCsvAmount,
  parseDateWithFormat,
  parseDecimalMinor,
  previewCsv,
  REASON_KEYS,
  signedAmount,
  splitRecords,
} from './bank';
import type { CsvMapping } from '../api/bank';

const GERMAN_IBAN = 'DE89370400440532013000';

function mapping(overrides: Partial<CsvMapping> = {}): CsvMapping {
  return {
    delimiter: ';',
    skip_rows: 0,
    has_header: true,
    decimal: ',',
    date_format: 'DD.MM.YYYY',
    booking_date: '#1',
    value_date: null,
    amount: '#3',
    debit: null,
    credit: null,
    currency: null,
    counterparty_name: '#2',
    counterparty_iban: null,
    remittance: ['#4'],
    bank_reference: null,
    ...overrides,
  };
}

describe('IBAN', () => {
  it('groups in fours and normalises spaces and case', () => {
    expect(formatIban('de89 3704 0044 0532 0130 00')).toBe('DE89 3704 0044 0532 0130 00');
    expect(formatIban(GERMAN_IBAN)).toBe('DE89 3704 0044 0532 0130 00');
  });

  it('accepts a valid IBAN and rejects a wrong check digit or shape', () => {
    expect(ibanProblem(GERMAN_IBAN)).toBeNull();
    expect(ibanProblem('de89 3704 0044 0532 0130 00')).toBeNull();
    expect(ibanProblem('DE88370400440532013000')).toBe('checksum');
    expect(ibanProblem('DE89')).toBe('format');
    expect(ibanProblem('  ')).toBe('empty');
  });
});

describe('money', () => {
  it('formats minor units with the currency exponent (JPY 0, EUR 2, BHD 3)', () => {
    expect(formatMinorPlain(123456n, 'JPY', 'en-GB')).toBe('123,456');
    expect(formatMinorPlain(123456n, 'EUR', 'en-GB')).toBe('1,234.56');
    expect(formatMinorPlain(123456n, 'BHD', 'en-GB')).toBe('123.456');
    expect(formatMinorPlain(123456n, 'EUR', 'de-DE')).toBe('1.234,56');
  });

  it('spells the sign of a signed amount', () => {
    expect(signedAmount('250000', 'EUR', 'en-GB')).toEqual({ text: '+2,500.00', sign: 'in' });
    expect(signedAmount('-1995', 'EUR', 'en-GB')).toEqual({ text: '−19.95', sign: 'out' });
    expect(signedAmount('0', 'EUR', 'en-GB').sign).toBe('zero');
  });

  it('parses a typed decimal into minor units, strictly', () => {
    expect(parseDecimalMinor('1250.00', 'EUR')).toBe(125000n);
    expect(parseDecimalMinor('1,250.50', 'EUR')).toBe(125050n);
    expect(parseDecimalMinor('-12.5', 'EUR')).toBe(-1250n);
    expect(parseDecimalMinor('1250', 'JPY')).toBe(1250n);
    expect(parseDecimalMinor('١٢٫٥', 'EUR')).toBe(1250n);
    // A comma is never read as a decimal point: "1250,50" is refused, not 125050.
    expect(parseDecimalMinor('1250,50', 'EUR')).toBeNull();
    expect(parseDecimalMinor('12.345', 'EUR')).toBeNull();
    expect(parseDecimalMinor('12.3456', 'BHD')).toBeNull();
    expect(parseDecimalMinor('abc', 'EUR')).toBeNull();
    expect(parseDecimalMinor('', 'EUR')).toBeNull();
  });
});

describe('line status', () => {
  it('takes the server word when it is one of the four', () => {
    expect(lineStatus({ status: 'ignored', matched: null, suggestions: [] })).toBe('ignored');
    expect(lineStatus({ status: 'suggested', matched: null, suggestions: [{}] })).toBe('suggested');
  });

  it('derives matched, suggested and unmatched from the fields when the word is unknown', () => {
    expect(lineStatus({ status: '', matched: { kind: 'entry', id: 'e1' }, suggestions: [] })).toBe('matched');
    expect(lineStatus({ status: '', matched: null, suggestions: [{ kind: 'rule' }] })).toBe('suggested');
    expect(lineStatus({ status: '', matched: null, suggestions: [] })).toBe('unmatched');
  });
});

describe('filters to query', () => {
  it('sends no status for All, and only the set filters', () => {
    expect(filtersToQuery(EMPTY_FILTERS, 'acc_1')).toEqual({ account_id: 'acc_1', limit: 50 });
  });

  it('maps status, trimmed search, valid dates and the cursor', () => {
    const query = filtersToQuery({ status: 'suggested', search: '  rent ', from: '2026-10-01', to: '2026-10-31' }, 'acc_1', { after: 'tx_9', limit: 20 });
    expect(query).toEqual({ account_id: 'acc_1', status: 'suggested', search: 'rent', from: '2026-10-01', to: '2026-10-31', limit: 20, after: 'tx_9' });
  });

  it('leaves out a date that is not a day', () => {
    expect(filtersToQuery({ ...EMPTY_FILTERS, from: '01.10.2026' }, 'acc_1')).toEqual({ account_id: 'acc_1', limit: 50 });
  });
});

describe('journal preview', () => {
  it('debits the bank on an inflow and credits the counterparty', () => {
    expect(journalPreview({ bankAccount: '1010', counterparty: '2000', amountMinor: 50000n, currency: 'EUR' })).toEqual([
      { account: '1010', side: 'debit', minor: 50000n, currency: 'EUR' },
      { account: '2000', side: 'credit', minor: 50000n, currency: 'EUR' },
    ]);
  });

  it('credits the bank on an outflow, with the counterparty debited', () => {
    const lines = journalPreview({ bankAccount: '1010', counterparty: '6000', amountMinor: -1995n, currency: 'EUR' });
    expect(lines.map((l) => [l.account, l.side, l.minor])).toEqual([
      ['1010', 'credit', 1995n],
      ['6000', 'debit', 1995n],
    ]);
    const debits = lines.filter((l) => l.side === 'debit').reduce((sum, l) => sum + l.minor, 0n);
    const credits = lines.filter((l) => l.side === 'credit').reduce((sum, l) => sum + l.minor, 0n);
    expect(debits).toBe(credits);
  });

  it('gives no lines for a zero amount', () => {
    expect(journalPreview({ bankAccount: '1010', counterparty: '6000', amountMinor: 0n, currency: 'EUR' })).toEqual([]);
  });
});

describe('rules and reasons', () => {
  it('words each condition that is set', () => {
    expect(conditionParts({ contains: 'Rent', counterparty_iban: GERMAN_IBAN, amount_min: -5000, amount_max: null })).toEqual([
      { kind: 'contains', text: 'Rent' },
      { kind: 'iban', text: 'DE89 3704 0044 0532 0130 00' },
      { kind: 'amount-min', minor: -5000n },
    ]);
  });

  it('picks the distinctive word of a remittance, else the IBAN', () => {
    expect(distinctiveWord('Invoice RE-2026-0042 Halden Supplies', null)).toBe('Supplies');
    expect(distinctiveWord('Payment 12 13', GERMAN_IBAN)).toBe(GERMAN_IBAN);
  });

  it('maps the reason codes the server sends to keys', () => {
    expect(REASON_KEYS['amount-equal']).toBe('bank.reason.amount_equal');
    expect(REASON_KEYS['rule-matches']).toBe('bank.reason.rule_matches');
  });
});

describe('account form problems', () => {
  it('reports each missing or invalid field', () => {
    expect(accountDraftProblems({ name: '', iban: 'DE88370400440532013000', currency: 'EU', ledgerAccount: '', opening: '12,5', openingDate: '' })).toEqual({
      name: 'required',
      iban: 'checksum',
      currency: 'format',
      ledger: 'required',
      opening: 'invalid',
      openingDate: 'required',
    });
    expect(accountDraftProblems({ name: 'Main', iban: GERMAN_IBAN, currency: 'EUR', ledgerAccount: '1010', opening: '0.00', openingDate: '2026-10-01' })).toEqual({});
  });
});

describe('CSV preview', () => {
  it('splits quoted fields with delimiters, doubled quotes and line breaks', () => {
    const text = 'a;"b;c";"say ""hi"""\r\n"multi\nline";x;y\n\n';
    expect(splitRecords(text, ';')).toEqual([
      ['a', 'b;c', 'say "hi"'],
      ['multi\nline', 'x', 'y'],
    ]);
  });

  it('parses dates strictly in the mapping format', () => {
    expect(parseDateWithFormat('05.10.2026', 'DD.MM.YYYY')).toBe('2026-10-05');
    expect(parseDateWithFormat('10/05/2026', 'MM/DD/YYYY')).toBe('2026-10-05');
    expect(parseDateWithFormat('31.02.2026', 'DD.MM.YYYY')).toBeNull();
    expect(parseDateWithFormat('2026-10-05', 'DD.MM.YYYY')).toBeNull();
  });

  it('reads German amounts with a comma decimal and a point for thousands', () => {
    expect(parseCsvAmount('-1.234,56', ',', 'EUR')).toBe(-123456n);
    expect(parseCsvAmount('12,5', ',', 'EUR')).toBe(1250n);
    expect(parseCsvAmount('12,345', ',', 'EUR')).toBeNull();
  });

  it('previews the first rows with the mapping, skipping lines and the header', () => {
    const text = [
      'Kontoauszug Oktober',
      'Buchung;Name;Betrag;Verwendungszweck',
      '01.10.2026;Halden Supplies GmbH;-1.190,00;RE-2026-0042',
      '02.10.2026;Kunde;2.500,00;Honorar Oktober',
      '31.02.2026;Kaputt;1,00;Fehler',
    ].join('\n');
    const preview = previewCsv(text, mapping({ skip_rows: 1 }), 'EUR', 5);
    expect(preview.headers).toEqual(['Buchung', 'Name', 'Betrag', 'Verwendungszweck']);
    expect(preview.columns[0]).toEqual({ value: '#1', label: 'Buchung' });
    expect(preview.rows.map((r) => [r.bookingDate, r.counterparty, r.amountMinor, r.remittance, r.problem])).toEqual([
      ['2026-10-01', 'Halden Supplies GmbH', -119000n, 'RE-2026-0042', null],
      ['2026-10-02', 'Kunde', 250000n, 'Honorar Oktober', null],
      [null, 'Kaputt', 100n, 'Fehler', 'booking-date'],
    ]);
  });

  it('marks a split debit and credit row and reads the signed difference', () => {
    const text = 'Date,Out,In,Ref\n2026-10-03,12.50,,A1\n2026-10-04,,40.00,B2';
    const preview = previewCsv(text, mapping({ delimiter: ',', decimal: '.', date_format: 'YYYY-MM-DD', amount: null, debit: '#2', credit: '#3', counterparty_name: null, remittance: ['#4'] }), 'EUR');
    expect(preview.rows.map((r) => r.amountMinor)).toEqual([-1250n, 4000n]);
  });
});

describe('mapping', () => {
  it('needs a booking date and exactly one amount form', () => {
    expect(mappingProblem(mapping())).toBeNull();
    expect(mappingProblem(mapping({ booking_date: '' }))).toBe('booking-date');
    expect(mappingProblem(mapping({ amount: null }))).toBe('amount-columns');
    expect(mappingProblem(mapping({ amount: '#3', debit: '#2', credit: '#3' }))).toBe('amount-columns');
    expect(mappingProblem(mapping({ delimiter: '|' }))).toBe('delimiter');
  });

  it('guesses a delimiter, the date format and columns from a sample', () => {
    const sample = 'Buchungstag,Empfaenger,Betrag,Verwendungszweck,IBAN\n05.10.2026,Halden,-19,95,Rent,DE89370400440532013000';
    const guess = guessMapping(sample);
    expect(guess.delimiter).toBe(',');
    expect(guess.booking_date).toBe('#1');
    expect(guess.remittance).toEqual(['#4']);
    expect(guess.date_format).toBe('DD.MM.YYYY');
    expect(guess.has_header).toBe(true);
  });
});

describe('format detection', () => {
  it('recognises camt.053, MT940 and falls back to CSV', () => {
    expect(detectFormat('<?xml version="1.0"?>\n<Document xmlns="urn:iso:std:iso:20022:tech:xsd:camt.053.001.02"><BkToCstmrStmt>')).toBe('camt053');
    expect(detectFormat('{1:F01BANKDEFF}{2:I940}\n:20:STARTUMS\n:25:DE89')).toBe('mt940');
    expect(detectFormat(':20:REF\n:25:X')).toBe('mt940');
    expect(detectFormat('Date;Amount\n01.10.2026;1,00')).toBe('csv');
  });
});
