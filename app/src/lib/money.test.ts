import { describe, expect, it } from 'vitest';
import { currencyExponent, formatMinor, parseMajorToMinor, toPlainMajor } from './money';

describe('currencyExponent', () => {
  it('uses each currency own decimals', () => {
    expect(currencyExponent('JPY')).toBe(0);
    expect(currencyExponent('EUR')).toBe(2);
    expect(currencyExponent('BHD')).toBe(3);
  });
});

describe('formatMinor', () => {
  it('formats with grouping and the currency decimals (en-US)', () => {
    expect(formatMinor('123456', 'EUR', 'en-US')).toBe('1,234.56');
    expect(formatMinor('5', 'EUR', 'en-US')).toBe('0.05');
    expect(formatMinor('1234', 'JPY', 'en-US')).toBe('1,234');
    expect(formatMinor('12345', 'BHD', 'en-US')).toBe('12.345');
  });

  it('uses the locale decimal and grouping separators', () => {
    expect(formatMinor('123456789', 'EUR', 'de-DE')).toBe('1.234.567,89');
  });

  it('prefixes negatives with a minus sign', () => {
    expect(formatMinor('-250', 'EUR', 'en-US')).toBe('−2.50');
  });

  it('is exact beyond float precision', () => {
    expect(formatMinor('9007199254740993567', 'EUR', 'en-US')).toBe('90,071,992,547,409,935.67');
  });
});

describe('parseMajorToMinor', () => {
  it('parses to minor units per currency exponent', () => {
    expect(parseMajorToMinor('12.34', 'EUR')).toBe(1234n);
    expect(parseMajorToMinor('1,234.5', 'EUR')).toBe(123450n);
    expect(parseMajorToMinor('1500', 'JPY')).toBe(1500n);
    expect(parseMajorToMinor('1.234', 'BHD')).toBe(1234n);
    expect(parseMajorToMinor('.5', 'EUR')).toBe(50n);
  });

  it('rejects text that is not a plain amount', () => {
    expect(parseMajorToMinor('', 'EUR')).toBeNull();
    expect(parseMajorToMinor('abc', 'EUR')).toBeNull();
    expect(parseMajorToMinor('1.2.3', 'EUR')).toBeNull();
    expect(parseMajorToMinor('-5', 'EUR')).toBeNull();
    expect(parseMajorToMinor('1.234', 'EUR')).toBeNull(); // more decimals than EUR has
    expect(parseMajorToMinor('1.5', 'JPY')).toBeNull();
    expect(parseMajorToMinor('1.2345', 'BHD')).toBeNull();
  });
});

describe('toPlainMajor', () => {
  it('gives CSV-ready major units without grouping', () => {
    expect(toPlainMajor('123456', 'EUR')).toBe('1234.56');
    expect(toPlainMajor('-5', 'EUR')).toBe('-0.05');
    expect(toPlainMajor('1234', 'JPY')).toBe('1234');
  });
});
