import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { ar } from './ar';
import { de } from './de';
import { en, type MessageKey } from './en';
import { id } from './id';
import { accountName, currentLocale, formatDate, formatPeriod, LOCALES, setLocale, t, tRich, type LocaleCode } from './index';
import { CHART_NAMES } from './chart-names.generated';
import { loadCatalogues, renderChartNames, TEMPLATES, OUTPUT } from '../../scripts/gen-i18n.mjs';
import { formatMinor, currencyExponent, parseMajorToMinor } from '../lib/money';

const CATALOGUES: Record<LocaleCode, Record<string, string>> = { en, de, id, ar };
const KEYS = Object.keys(en) as MessageKey[];
const PLACEHOLDER = /\{(\w+)\}/g;

function placeholders(text: string): string[] {
  return [...text.matchAll(PLACEHOLDER)].map((m) => m[1]).sort();
}

describe('message catalogues', () => {
  it.each(['de', 'id', 'ar'] as const)('%s has every English key, with a value', (code) => {
    const missing = KEYS.filter((key) => !(key in CATALOGUES[code]));
    expect(missing).toEqual([]);
    const empty = KEYS.filter((key) => !CATALOGUES[code][key]?.trim());
    expect(empty).toEqual([]);
  });

  it('has no key outside the English catalogue', () => {
    for (const code of ['de', 'id', 'ar'] as const) {
      const extra = Object.keys(CATALOGUES[code]).filter((key) => !(key in en));
      expect(extra, code).toEqual([]);
    }
  });

  it('keeps the same placeholders in every language', () => {
    for (const key of KEYS) {
      const expected = placeholders(en[key]);
      for (const code of ['de', 'id', 'ar'] as const) {
        expect(placeholders(CATALOGUES[code][key]), `${code} ${key}`).toEqual(expected);
      }
    }
  });

  it('offers exactly the four launch languages, each with a catalogue', () => {
    expect(LOCALES.map((locale) => locale.code)).toEqual(['en', 'de', 'id', 'ar']);
    expect(LOCALES.find((locale) => locale.code === 'ar')?.dir).toBe('rtl');
    expect(LOCALES.map((locale) => locale.name)).toEqual(['English', 'Deutsch', 'Bahasa Indonesia', 'العربية']);
  });

  it('fills placeholders and keeps a missing one visible', () => {
    setLocale('en');
    expect(t('overview.empty_title', { period: 'Oct 2026' })).toBe('No entries in Oct 2026 yet');
    expect(t('overview.empty_title')).toBe('No entries in {period} yet');
  });

  it('switches language and reports the direction', () => {
    setLocale('ar');
    expect(currentLocale().dir).toBe('rtl');
    expect(t('nav.journal')).toBe(ar['nav.journal']);
    setLocale('de');
    expect(currentLocale().dir).toBe('ltr');
    expect(t('nav.journal')).toBe(de['nav.journal']);
    setLocale('en');
  });

  it('renders rich messages with elements in place of placeholders', () => {
    setLocale('en');
    const parts = tRich('entry.reverse_body', { ref: 'FY2026-0001', reversal: 'REV' });
    expect(parts.join('')).toContain('FY2026-0001');
    expect(parts).toContain('REV');
  });
});

describe('chart names built from templates/', () => {
  it('the committed generated module is up to date with templates/i18n', () => {
    const rendered = renderChartNames(loadCatalogues(TEMPLATES));
    expect(readFileSync(OUTPUT, 'utf8')).toBe(rendered);
  });

  it('every name key used by the generic SME chart exists in all four languages', () => {
    const chart = readFileSync(fileURLToPath(new URL('../../../templates/generic-sme.toml', import.meta.url)), 'utf8');
    const keys = [...chart.matchAll(/name = "([^"]+)"/g)].map((m) => m[1]);
    expect(keys.length).toBeGreaterThan(20);
    for (const code of ['en', 'de', 'id', 'ar'] as const) {
      const missing = keys.filter((key) => !(key in (CHART_NAMES as Record<string, Record<string, string>>)[code]));
      expect(missing, code).toEqual([]);
    }
  });

  it('translates a template account name, and humanises a key the templates do not carry', () => {
    setLocale('de');
    expect(accountName('account.generic_sme.accounts_receivable')).toBe(CHART_NAMES.de['account.generic_sme.accounts_receivable']);
    setLocale('en');
    expect(accountName('account.generic_sme.accounts_receivable')).toBe('Accounts receivable');
    expect(accountName('account.cash_at_bank')).toBe('Cash at bank');
    setLocale('ar');
    expect(accountName('account.generic_sme.cash')).toBe(CHART_NAMES.ar['account.generic_sme.cash']);
    setLocale('en');
  });
});

describe('locale formatting', () => {
  it('uses each currency’s own decimals (JPY 0, EUR 2, BHD 3)', () => {
    expect(currencyExponent('JPY')).toBe(0);
    expect(currencyExponent('EUR')).toBe(2);
    expect(currencyExponent('BHD')).toBe(3);
    expect(formatMinor('123456', 'JPY', 'en-GB')).toBe('123,456');
    expect(formatMinor('123456', 'BHD', 'en-GB')).toBe('123.456');
    expect(formatMinor('123456', 'EUR', 'de-DE')).toBe('1.234,56');
  });

  it('keeps Western digits in Arabic amounts and parses Arabic digits back', () => {
    const formatted = formatMinor('123456', 'EUR', 'ar-u-nu-latn');
    // CLDR's Latin numbering for Arabic: Western digits with the usual separators.
    expect(formatted).toBe('1,234.56');
    expect(/[٠-٩]/.test(formatted)).toBe(false);
    expect(parseMajorToMinor('١٢٣٫٤٥', 'EUR')).toBe(12345n);
    expect(parseMajorToMinor('1,234.50', 'EUR')).toBe(123450n);
  });

  it('formats dates and periods in the language in use', () => {
    setLocale('en');
    expect(formatDate('2026-10-03')).toMatch(/Oct/);
    expect(formatPeriod('2026-10')).toMatch(/2026/);
    setLocale('de');
    expect(formatDate('2026-10-03')).toMatch(/Okt/);
    setLocale('en');
    expect(formatDate('not a date')).toBe('not a date');
  });
});
