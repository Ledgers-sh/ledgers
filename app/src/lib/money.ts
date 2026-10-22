/**
 * Money is never a float. Amounts arrive as integer minor-unit strings and are
 * handled as BigInt. Each currency's own decimals come from Intl (JPY 0,
 * EUR 2, BHD 3). Codes Intl does not know fall back to 2 digits.
 */

const exponentCache = new Map<string, number>();

/** Number of minor-unit digits for an ISO 4217 code (JPY 0, EUR 2, BHD 3). */
export function currencyExponent(currency: string): number {
  const cached = exponentCache.get(currency);
  if (cached !== undefined) return cached;
  let digits = 2;
  try {
    const resolved = new Intl.NumberFormat('en', { style: 'currency', currency }).resolvedOptions();
    digits = resolved.maximumFractionDigits ?? 2;
  } catch {
    digits = 2;
  }
  exponentCache.set(currency, digits);
  return digits;
}

/** Parses a user-typed major amount ("1,234.50", "12.5") into BigInt minor units. Returns null when invalid. */
export function parseMajorToMinor(input: string, currency: string): bigint | null {
  const trimmed = input.trim().replace(/[\s  ]/g, '');
  if (trimmed === '') return null;
  const exponent = currencyExponent(currency);
  // "." is the decimal separator; "," is grouping and is ignored.
  const parts = trimmed.replace(/,/g, '').split('.');
  if (parts.length > 2) return null;
  const [whole, fraction = ''] = parts;
  if (!/^\d*$/.test(whole)) return null;
  if (!/^\d*$/.test(fraction)) return null;
  if (fraction.length > exponent) return null;
  const digits = (whole === '' ? '0' : whole) + fraction.padEnd(exponent, '0');
  return BigInt(digits);
}

/** Formats BigInt-safe minor units with locale grouping and decimal separators. */
export function formatMinor(minor: string | bigint, currency: string, locale = 'en-US'): string {
  const value = typeof minor === 'bigint' ? minor : BigInt(minor);
  const exponent = currencyExponent(currency);
  const negative = value < 0n;
  const abs = negative ? -value : value;
  const digits = abs.toString().padStart(exponent + 1, '0');
  const intPart = digits.slice(0, digits.length - exponent);
  const fracPart = exponent > 0 ? digits.slice(digits.length - exponent) : '';

  const separators = new Intl.NumberFormat(locale).formatToParts(1000.1);
  const group = separators.find((p) => p.type === 'group')?.value ?? ',';
  const decimal = separators.find((p) => p.type === 'decimal')?.value ?? '.';
  const groupedInt = BigInt(intPart).toString().replace(/\B(?=(\d{3})+(?!\d))/g, group);
  const number = exponent > 0 ? `${groupedInt}${decimal}${fracPart}` : groupedInt;
  return `${negative ? '−' : ''}${number}`;
}

/** Formats minor units as a plain major-unit string for CSV exports ("1234.50", no grouping). */
export function toPlainMajor(minor: string | bigint, currency: string): string {
  const value = typeof minor === 'bigint' ? minor : BigInt(minor);
  const exponent = currencyExponent(currency);
  const negative = value < 0n;
  const digits = (negative ? -value : value).toString().padStart(exponent + 1, '0');
  const intPart = digits.slice(0, digits.length - exponent);
  const fracPart = exponent > 0 ? `.${digits.slice(digits.length - exponent)}` : '';
  return `${negative ? '-' : ''}${intPart}${fracPart}`;
}
