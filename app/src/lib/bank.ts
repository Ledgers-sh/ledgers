/**
 * Pure helpers of the bank screens: IBANs, amounts with each currency's own
 * decimals, line status, filters to query, the journal preview of a line,
 * the CSV preview, format detection and rule wording. No I/O here; the
 * screens call these and the tests cover them.
 *
 * Money is never a float: amounts are BigInt minor units, and a typed decimal
 * is parsed strictly (no guessing between "1,250" and "1.250").
 */
import { currencyExponent, normaliseDigits, toPlainMajor } from './money';
import type { CsvMapping } from '../api/bank';

// --- IBAN ------------------------------------------------------------------

/** Upper case, no spaces: the form the server stores and compares. */
export function normaliseIban(input: string): string {
  return input.replace(/\s+/g, '').toUpperCase();
}

/** Groups of four, as printed on statements: "DE89 3704 0044 0532 0130 00". */
export function formatIban(iban: string): string {
  return normaliseIban(iban).replace(/(.{4})(?=.)/g, '$1 ');
}

/** The first problem with an IBAN, or null when it is well formed with valid check digits. */
export function ibanProblem(input: string): 'empty' | 'format' | 'checksum' | null {
  const iban = normaliseIban(input);
  if (iban === '') return 'empty';
  if (!/^[A-Z]{2}[0-9]{2}[A-Z0-9]{11,30}$/.test(iban)) return 'format';
  const rearranged = iban.slice(4) + iban.slice(0, 4);
  let remainder = 0;
  for (const ch of rearranged) {
    const value = ch >= 'A' && ch <= 'Z' ? String(ch.charCodeAt(0) - 55) : ch;
    for (const digit of value) remainder = (remainder * 10 + Number(digit)) % 97;
  }
  return remainder === 1 ? null : 'checksum';
}

// --- amounts ---------------------------------------------------------------

/**
 * A typed decimal amount ("1250.00", "-12.5", "1,250.00") as BigInt minor units.
 * Null when it is not a plain decimal with at most the currency's digits after
 * the point. A comma is accepted only as a thousands separator, so "1250,50"
 * (German style) is refused rather than read as 125050.
 */
export function parseDecimalMinor(input: string, currency: string): bigint | null {
  let text = normaliseDigits(input).trim().replace(/[\s  ]/g, '').replace(/−/g, '-');
  if (text === '') return null;
  const negative = text.startsWith('-');
  if (negative || text.startsWith('+')) text = text.slice(1);
  if (/^\d{1,3}(,\d{3})+(\.\d+)?$/.test(text)) text = text.replace(/,/g, '');
  if (!/^\d+(\.\d+)?$|^\.\d+$/.test(text)) return null;
  const [whole, fraction = ''] = text.split('.');
  const exponent = currencyExponent(currency);
  if (fraction.length > exponent) return null;
  const digits = (whole === '' ? '0' : whole) + fraction.padEnd(exponent, '0');
  const value = BigInt(digits);
  return negative ? -value : value;
}

/** A decimal amount as the server takes it: plain major units in the account's currency, e.g. "1250.00". */
export function plainMajor(minor: bigint, currency: string): string {
  return toPlainMajor(minor, currency);
}

/** An amount formatted in the reader's language with its own decimals; the sign is spelt out (+ or −). */
export function signedAmount(minor: string | bigint, currency: string, locale: string): { text: string; sign: 'in' | 'out' | 'zero' } {
  const value = typeof minor === 'bigint' ? minor : BigInt(minor);
  const abs = value < 0n ? -value : value;
  const body = formatMinorPlain(abs, currency, locale);
  if (value > 0n) return { text: `+${body}`, sign: 'in' };
  if (value < 0n) return { text: `−${body}`, sign: 'out' };
  return { text: body, sign: 'zero' };
}

/** Grouped, with the currency's decimals, no sign. Uses the same separators Intl gives the locale. */
export function formatMinorPlain(abs: bigint, currency: string, locale: string): string {
  const exponent = currencyExponent(currency);
  const digits = abs.toString().padStart(exponent + 1, '0');
  const intPart = digits.slice(0, digits.length - exponent);
  const fracPart = exponent > 0 ? digits.slice(digits.length - exponent) : '';
  const parts = new Intl.NumberFormat(locale).formatToParts(1000.1);
  const group = parts.find((p) => p.type === 'group')?.value ?? ',';
  const decimal = parts.find((p) => p.type === 'decimal')?.value ?? '.';
  const grouped = BigInt(intPart).toString().replace(/\B(?=(\d{3})+(?!\d))/g, group);
  return exponent > 0 ? `${grouped}${decimal}${fracPart}` : grouped;
}

// --- statuses and filters ---------------------------------------------------

export const LINE_STATUSES = ['unmatched', 'suggested', 'matched', 'ignored'] as const;
export type LineStatus = (typeof LINE_STATUSES)[number];

export interface StatusSource {
  status: string;
  matched: unknown;
  suggestions: unknown[];
}

/**
 * The status a line shows. The server's word wins when it is one of the four;
 * otherwise it is derived: matched when linked, else suggested when a
 * suggestion exists, else unmatched. Ignored lines are the server's word only.
 */
export function lineStatus(line: StatusSource): LineStatus {
  if ((LINE_STATUSES as readonly string[]).includes(line.status)) return line.status as LineStatus;
  if (line.matched) return 'matched';
  return line.suggestions.length > 0 ? 'suggested' : 'unmatched';
}

export interface TransactionFilters {
  /** "all" sends no status. */
  status: 'all' | LineStatus;
  search: string;
  /** YYYY-MM-DD or empty. */
  from: string;
  /** YYYY-MM-DD or empty. */
  to: string;
}

export const EMPTY_FILTERS: TransactionFilters = { status: 'all', search: '', from: '', to: '' };

export interface TransactionsQuery {
  account_id: string;
  status?: LineStatus;
  search?: string;
  from?: string;
  to?: string;
  limit: number;
  after?: string;
}

const ISO_DAY = /^\d{4}-\d{2}-\d{2}$/;

/** The list call for the current filters. Empty text and dates are left out, not sent blank. */
export function filtersToQuery(filters: TransactionFilters, accountId: string, options: { after?: string; limit?: number } = {}): TransactionsQuery {
  const query: TransactionsQuery = { account_id: accountId, limit: options.limit ?? 50 };
  if (filters.status !== 'all') query.status = filters.status;
  const search = filters.search.trim();
  if (search !== '') query.search = search;
  if (ISO_DAY.test(filters.from)) query.from = filters.from;
  if (ISO_DAY.test(filters.to)) query.to = filters.to;
  if (options.after) query.after = options.after;
  return query;
}

// --- journal preview ----------------------------------------------------------

export interface PreviewLine {
  account: string;
  side: 'debit' | 'credit';
  minor: bigint;
  currency: string;
}

/**
 * The entry a line would post: the bank's ledger account on one side, the
 * counterparty on the other, both for the line's amount. An inflow debits the
 * bank account; an outflow credits it. Zero amounts give no lines.
 */
export function journalPreview(input: { bankAccount: string; counterparty: string; amountMinor: bigint; currency: string }): PreviewLine[] {
  const { amountMinor, currency } = input;
  if (amountMinor === 0n) return [];
  const abs = amountMinor < 0n ? -amountMinor : amountMinor;
  const inflow = amountMinor > 0n;
  return [
    { account: input.bankAccount, side: inflow ? 'debit' : 'credit', minor: abs, currency },
    { account: input.counterparty, side: inflow ? 'credit' : 'debit', minor: abs, currency },
  ];
}

// --- suggestions ----------------------------------------------------------------

/** Message keys of the reason codes the server sends. A code without a key is left out of the words, never shown raw. */
export const REASON_KEYS: Record<string, 'bank.reason.rule_matches' | 'bank.reason.amount_equal' | 'bank.reason.vendor_iban' | 'bank.reason.vendor_name' | 'bank.reason.invoice_number' | 'bank.reason.date_window' | 'bank.reason.date_close' | 'bank.reason.counterparty_named'> = {
  'rule-matches': 'bank.reason.rule_matches',
  'amount-equal': 'bank.reason.amount_equal',
  'vendor-iban': 'bank.reason.vendor_iban',
  'vendor-name': 'bank.reason.vendor_name',
  'invoice-number': 'bank.reason.invoice_number',
  'date-window': 'bank.reason.date_window',
  'date-close': 'bank.reason.date_close',
  'description-names-counterparty': 'bank.reason.counterparty_named',
};

// --- rules ----------------------------------------------------------------------

export interface RuleConditionsLike {
  contains?: string | null;
  counterparty_iban?: string | null;
  amount_min?: number | null;
  amount_max?: number | null;
}

export type ConditionPart =
  | { kind: 'contains'; text: string }
  | { kind: 'iban'; text: string }
  | { kind: 'amount-min'; minor: bigint }
  | { kind: 'amount-max'; minor: bigint };

/** The conditions of a rule as parts to word in the screen: each part is one clause, and "all of" joins them. */
export function conditionParts(conditions: RuleConditionsLike): ConditionPart[] {
  const parts: ConditionPart[] = [];
  if (conditions.contains) parts.push({ kind: 'contains', text: conditions.contains });
  if (conditions.counterparty_iban) parts.push({ kind: 'iban', text: formatIban(conditions.counterparty_iban) });
  if (conditions.amount_min !== null && conditions.amount_min !== undefined) parts.push({ kind: 'amount-min', minor: BigInt(conditions.amount_min) });
  if (conditions.amount_max !== null && conditions.amount_max !== undefined) parts.push({ kind: 'amount-max', minor: BigInt(conditions.amount_max) });
  return parts;
}

const STOP_WORDS = new Set(['invoice', 'payment', 'transfer', 'reference', 'ref', 'rechnung', 'zahlung', 'the', 'and', 'for', 'from', 'with', 'this']);

/**
 * A distinctive word of a remittance, for a rule's "contains" clause: the
 * longest word of four or more letters that is not a common payment word.
 * Falls back to the counterparty IBAN when the remittance has none.
 */
export function distinctiveWord(remittance: string, counterpartyIban: string | null): string {
  const words = remittance
    .split(/[^\p{L}\p{N}]+/u)
    .filter((word) => word.length >= 4 && !/^\d+$/.test(word) && !STOP_WORDS.has(word.toLowerCase()));
  if (words.length > 0) {
    return words.reduce((best, word) => (word.length > best.length ? word : best));
  }
  return counterpartyIban ? normaliseIban(counterpartyIban) : '';
}

// --- account form -------------------------------------------------------------------

export interface AccountDraftProblems {
  name?: 'required';
  iban?: 'empty' | 'format' | 'checksum';
  currency?: 'format';
  ledger?: 'required';
  opening?: 'invalid';
  openingDate?: 'required';
}

export function accountDraftProblems(draft: {
  name: string;
  iban: string;
  currency: string;
  ledgerAccount: string;
  opening: string;
  openingDate: string;
}): AccountDraftProblems {
  const problems: AccountDraftProblems = {};
  if (draft.name.trim() === '') problems.name = 'required';
  const iban = ibanProblem(draft.iban);
  if (iban) problems.iban = iban;
  if (!/^[A-Z]{3}$/.test(draft.currency)) problems.currency = 'format';
  if (draft.ledgerAccount === '') problems.ledger = 'required';
  if (parseDecimalMinor(draft.opening, /^[A-Z]{3}$/.test(draft.currency) ? draft.currency : 'EUR') === null) problems.opening = 'invalid';
  if (!ISO_DAY.test(draft.openingDate)) problems.openingDate = 'required';
  return problems;
}

// --- CSV ------------------------------------------------------------------------------

export const DATE_FORMATS = ['YYYY-MM-DD', 'DD.MM.YYYY', 'DD/MM/YYYY', 'MM/DD/YYYY'] as const;

/** The first problem with a mapping's own shape, as a key for the screen; null when it can be used. */
export function mappingProblem(mapping: CsvMapping): 'delimiter' | 'decimal' | 'date-format' | 'booking-date' | 'amount-columns' | null {
  if (!['\t', ';', ','].includes(mapping.delimiter)) return 'delimiter';
  if (mapping.decimal !== '.' && mapping.decimal !== ',') return 'decimal';
  if (!(DATE_FORMATS as readonly string[]).includes(mapping.date_format)) return 'date-format';
  if (mapping.booking_date.trim() === '') return 'booking-date';
  const single = mapping.amount !== null && mapping.amount !== undefined && mapping.amount !== '';
  const split = !!mapping.debit && !!mapping.credit;
  if (single === split) return 'amount-columns';
  return null;
}

/**
 * Splits CSV text into records. Quoted fields may hold the delimiter, line
 * breaks and doubled quotes. Blank lines are skipped.
 */
export function splitRecords(text: string, delimiter: string): string[][] {
  const records: string[][] = [];
  let row: string[] = [];
  let field = '';
  let quoted = false;
  let i = 0;
  const pushField = () => {
    row.push(field);
    field = '';
  };
  const pushRow = () => {
    pushField();
    if (!(row.length === 1 && row[0].trim() === '')) records.push(row);
    row = [];
  };
  while (i < text.length) {
    const ch = text[i];
    if (quoted) {
      if (ch === '"') {
        if (text[i + 1] === '"') {
          field += '"';
          i += 2;
          continue;
        }
        quoted = false;
      } else {
        field += ch;
      }
    } else if (ch === '"' && field === '') {
      quoted = true;
    } else if (ch === delimiter) {
      pushField();
    } else if (ch === '\n' || ch === '\r') {
      if (ch === '\r' && text[i + 1] === '\n') i += 1;
      pushRow();
    } else {
      field += ch;
    }
    i += 1;
  }
  if (field !== '' || row.length > 0) pushRow();
  return records;
}

/** A date in the mapping's format as YYYY-MM-DD, or null when it is not a real date. */
export function parseDateWithFormat(text: string, format: string): string | null {
  const value = text.trim();
  let year: number, month: number, day: number;
  let m: RegExpExecArray | null;
  if (format === 'YYYY-MM-DD') {
    m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
    if (!m) return null;
    [year, month, day] = [Number(m[1]), Number(m[2]), Number(m[3])];
  } else if (format === 'DD.MM.YYYY') {
    m = /^(\d{2})\.(\d{2})\.(\d{4})$/.exec(value);
    if (!m) return null;
    [day, month, year] = [Number(m[1]), Number(m[2]), Number(m[3])];
  } else if (format === 'DD/MM/YYYY') {
    m = /^(\d{2})\/(\d{2})\/(\d{4})$/.exec(value);
    if (!m) return null;
    [day, month, year] = [Number(m[1]), Number(m[2]), Number(m[3])];
  } else if (format === 'MM/DD/YYYY') {
    m = /^(\d{2})\/(\d{2})\/(\d{4})$/.exec(value);
    if (!m) return null;
    [month, day, year] = [Number(m[1]), Number(m[2]), Number(m[3])];
  } else {
    return null;
  }
  const check = new Date(Date.UTC(year, month - 1, day));
  if (check.getUTCFullYear() !== year || check.getUTCMonth() !== month - 1 || check.getUTCDate() !== day) return null;
  return `${String(year).padStart(4, '0')}-${String(month).padStart(2, '0')}-${String(day).padStart(2, '0')}`;
}

/** A decimal in the mapping's notation as BigInt minor units, or null. Thousands separators of the other notation are allowed. */
export function parseCsvAmount(text: string, decimal: string, currency: string): bigint | null {
  let value = normaliseDigits(text).trim().replace(/[\s ]/g, '').replace(/−/g, '-');
  if (value === '') return null;
  if (decimal === ',') {
    value = value.replace(/\./g, '').replace(',', '.');
  } else {
    value = value.replace(/,/g, '');
  }
  return parseDecimalMinor(value, currency);
}

export interface CsvPreviewRow {
  line: number;
  bookingDate: string | null;
  valueDate: string | null;
  amountMinor: bigint | null;
  currency: string;
  counterparty: string;
  remittance: string;
  /** The first field the row could not be read from, as a key for the screen; null when the row is good. */
  problem: 'booking-date' | 'amount' | 'columns' | null;
}

export interface CsvPreview {
  headers: string[] | null;
  /** Column choices: value is the #n reference the mapping stores, label the header text (or "Column n"). */
  columns: { value: string; label: string }[];
  rows: CsvPreviewRow[];
  /** Records in the file after skipping, including the header. */
  records: number;
}

/** A 1-based column reference, as the server reads "#n". */
export function columnRef(index: number): string {
  return `#${index + 1}`;
}

/** The index a "#n" reference names, or -1. Header names are not resolved here; the screen only writes #n references. */
function columnIndex(ref: string | null | undefined): number {
  if (!ref || !ref.startsWith('#')) return -1;
  const n = Number(ref.slice(1));
  return Number.isInteger(n) && n >= 1 ? n - 1 : -1;
}

/**
 * The first rows of a CSV read with a mapping, the way the server reads them:
 * skipped lines, then an optional header, then records. Dates and amounts that
 * do not read are marked, so the wizard can show them. Client-side only; the
 * import is still read by the server.
 */
export function previewCsv(text: string, mapping: CsvMapping, accountCurrency: string, limit = 5): CsvPreview {
  const all = splitRecords(text, mapping.delimiter);
  const afterSkip = all.slice(Math.max(0, mapping.skip_rows));
  const width = afterSkip.reduce((max, record) => Math.max(max, record.length), 0);
  const headers = mapping.has_header && afterSkip.length > 0 ? afterSkip[0] : null;
  const body = headers ? afterSkip.slice(1) : afterSkip;
  const columns = Array.from({ length: width }, (_, index) => ({
    value: columnRef(index),
    label: headers?.[index]?.trim() ? headers[index].trim() : `Column ${index + 1}`,
  }));
  const at = (record: string[], ref: string | null | undefined) => {
    const index = columnIndex(ref);
    return index >= 0 ? (record[index] ?? '') : null;
  };
  const rows: CsvPreviewRow[] = body.slice(0, limit).map((record, offset) => {
    const bookingRaw = at(record, mapping.booking_date) ?? '';
    const bookingDate = parseDateWithFormat(bookingRaw, mapping.date_format);
    const valueRaw = at(record, mapping.value_date);
    const valueDate = valueRaw === null || valueRaw === '' ? null : parseDateWithFormat(valueRaw, mapping.date_format);
    const currencyRaw = at(record, mapping.currency);
    const currency = currencyRaw && /^[A-Z]{3}$/.test(currencyRaw.trim()) ? currencyRaw.trim() : accountCurrency;
    let amountMinor: bigint | null = null;
    if (mapping.amount) {
      amountMinor = parseCsvAmount(at(record, mapping.amount) ?? '', mapping.decimal, currency);
    } else if (mapping.debit && mapping.credit) {
      const debit = at(record, mapping.debit) ?? '';
      const credit = at(record, mapping.credit) ?? '';
      const debitMinor = debit.trim() === '' ? 0n : parseCsvAmount(debit, mapping.decimal, currency);
      const creditMinor = credit.trim() === '' ? 0n : parseCsvAmount(credit, mapping.decimal, currency);
      amountMinor = debitMinor === null || creditMinor === null ? null : creditMinor - debitMinor;
    }
    const counterparty = [at(record, mapping.counterparty_name), at(record, mapping.counterparty_iban)].filter(Boolean).join(' ');
    const remittance = mapping.remittance
      .map((ref) => (at(record, ref) ?? '').trim())
      .filter((part) => part !== '')
      .join(' ');
    let problem: CsvPreviewRow['problem'] = null;
    if (bookingDate === null) problem = 'booking-date';
    else if (amountMinor === null) problem = 'amount';
    return {
      line: mapping.skip_rows + (headers ? 1 : 0) + offset + 1,
      bookingDate,
      valueDate,
      amountMinor,
      currency,
      counterparty: counterparty.trim(),
      remittance,
      problem,
    };
  });
  return { headers, columns, rows, records: afterSkip.length };
}

/** The format of an uploaded statement, read from its first bytes: camt.053 XML, MT940 (starting {1: or :20:), else CSV. */
export function detectFormat(text: string): 'camt053' | 'mt940' | 'csv' {
  const head = text.replace(/^﻿/, '').trimStart();
  if (/^\{1:/.test(head) || /^:20:/m.test(head.slice(0, 400))) return 'mt940';
  if (/^(?:<\?xml[^>]*\?>\s*)?(?:<!--[\s\S]*?-->\s*)*<(?:[\w.-]+:)?Document[\s>]/.test(head)) return 'camt053';
  return 'csv';
}


/**
 * A starting mapping for a sample file: the delimiter is the most common of
 * ; , and tab on the first line, and columns are matched by their header
 * words (date, amount, counterparty, reference). The person checks it in the
 * wizard before saving; nothing here is saved.
 */
export function guessMapping(text: string): CsvMapping {
  const firstLine = text.replace(/^\uFEFF/, '').split(/\r?\n/, 1)[0] ?? '';
  const counts = [';', ',', '\t'].map((d) => [d, firstLine.split(d).length - 1] as const);
  const delimiter = counts.reduce((best, cur) => (cur[1] > best[1] ? cur : best))[0];
  const records = splitRecords(text, delimiter).slice(0, 20);
  const headerRow = records[0] ?? [];
  const header = headerRow.map((cell) => cell.trim().toLowerCase());
  const find = (pattern: RegExp) => {
    const index = header.findIndex((cell) => pattern.test(cell));
    return index >= 0 ? columnRef(index) : null;
  };
  const bookingDate = find(/buchung|booking|book|datum|date/) ?? columnRef(0);
  const valueDate = find(/valuta|value/);
  const amount = find(/amount|betrag|umsatz|sum/);
  const debit = amount ? null : find(/debit|soll|ausgang|withdraw/);
  const credit = amount ? null : find(/credit|haben|eingang|deposit/);
  const counterpartyName = find(/name|empf|payee|counterparty|partner/);
  const counterpartyIban = find(/iban/);
  const remittance = header
    .map((cell, index) => (/verwend|purpose|remit|reference|text|description|beschreibung|zweck/.test(cell) ? columnRef(index) : null))
    .filter((ref): ref is string => ref !== null)
    .slice(0, 3);
  const bankReference = find(/^(ref|reference|referenz|bank ref)/);
  const sample = records.slice(1).flat().join(' ');
  // Amount-shaped tokens only: a date such as 05.10.2026 must not vote for a point decimal.
  const commaAmounts = sample.match(/(?<![\d.])-?\d{1,3}(?:\.\d{3})*,\d{2}(?![\d.])/g)?.length ?? 0;
  const pointAmounts = sample.match(/(?<![\d.,])-?\d+\.\d{2}(?![\d.])/g)?.length ?? 0;
  const decimal = commaAmounts > 0 && commaAmounts >= pointAmounts ? ',' : '.';
  const dateFormat = /\b\d{2}\.\d{2}\.\d{4}\b/.test(sample) ? 'DD.MM.YYYY' : /\b\d{4}-\d{2}-\d{2}\b/.test(sample) ? 'YYYY-MM-DD' : 'DD/MM/YYYY';
  return {
    delimiter,
    skip_rows: 0,
    has_header: headerRow.some((cell) => /[a-zA-Z]/.test(cell)),
    decimal,
    date_format: dateFormat,
    booking_date: bookingDate,
    value_date: valueDate,
    amount: amount ?? null,
    debit,
    credit,
    currency: find(/currency|währung|waehrung|ccy/),
    counterparty_name: counterpartyName,
    counterparty_iban: counterpartyIban,
    remittance,
    bank_reference: bankReference,
  };
}
