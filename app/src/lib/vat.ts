/**
 * Company profile checks the Settings screen runs before it sends a form.
 * The server runs the same rules (crates/ledgers-server/src/dispatch/settings_rules.rs)
 * and refuses anything these let through, so this is for quick, field-level messages.
 */

/** A VAT-ID pattern per country: `d` digit, `b` digit 0 or 1, `a` capital, `x` capital or digit, others literal. */
const VAT_PATTERNS: Record<string, string[]> = {
  AT: ['ATUd8'],
  BE: ['BEbd9'],
  DE: ['DEd9'],
  DK: ['DKd8'],
  ES: ['ESxd7x'],
  FR: ['FRxxd9'],
  GB: ['GBd9', 'GBd12', 'GBGDd3', 'GBHAd3'],
  IE: ['IEdxd5a', 'IEdxd5aa'],
  IT: ['ITd11'],
  NL: ['NLd9Bd2'],
  PL: ['PLd10'],
  SE: ['SEd12'],
};

/** Matches one pattern (see VAT_PATTERNS) against the whole text. */
export function matchesPattern(text: string, pattern: string): boolean {
  const tokens = pattern.match(/[a-zA-Z](?:\d+)?/g) ?? [];
  let pos = 0;
  for (const token of tokens) {
    const [cls, count] = [token[0], token.length > 1 ? Number(token.slice(1)) : 1];
    for (let i = 0; i < count; i += 1) {
      const c = text[pos];
      if (c === undefined) return false;
      const ok =
        cls === 'd' ? /[0-9]/.test(c) : cls === 'b' ? c === '0' || c === '1' : cls === 'a' ? /[A-Z]/.test(c) : cls === 'x' ? /[A-Z0-9]/.test(c) : c === cls;
      if (!ok) return false;
      pos += 1;
    }
  }
  return pos === text.length;
}

/** Upper-cases a VAT ID and drops the spaces, dots and dashes between its groups. */
export function normalizeVatId(raw: string): string {
  return raw.replace(/[ .\-\t]/g, '').toUpperCase();
}

/**
 * The reason a VAT ID does not fit its country, as a message key the screen
 * translates, or null when it fits (an empty VAT ID always fits).
 */
export type VatProblem = 'country_needed' | 'prefix' | 'format';

export function vatProblem(country: string, rawVatId: string): VatProblem | null {
  const vat = normalizeVatId(rawVatId);
  if (vat === '') return null;
  if (country === '') return 'country_needed';
  if (!vat.startsWith(country)) return 'prefix';
  const patterns = VAT_PATTERNS[country];
  if (patterns) return patterns.some((p) => matchesPattern(vat, p)) ? null : 'format';
  const body = vat.slice(country.length);
  return /^[A-Z0-9]{2,12}$/.test(body) ? null : 'format';
}

/** A contact address: one mailbox, no spaces, a dotted domain. Empty is allowed. */
export function emailProblem(raw: string): boolean {
  const value = raw.trim();
  if (value === '') return false;
  return !/^[^\s@]+@[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?(?:\.[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?)+$/.test(value) || value.length > 254;
}

/** A country code: empty, or two capital letters. */
export function countryProblem(raw: string): boolean {
  const value = raw.trim().toUpperCase();
  return value !== '' && !/^[A-Z]{2}$/.test(value);
}

/** A low-risk amount in a currency's decimals: whole, or with at most `exponent` decimals, above zero. */
export function amountProblem(raw: string, exponent: number): boolean {
  const value = raw.trim();
  const match = /^(\d+)(?:\.(\d+))?$/.exec(value);
  if (!match) return true;
  const decimals = match[2] ?? '';
  if (decimals.length > exponent) return true;
  return Number(match[1]) === 0 && !/[1-9]/.test(decimals);
}

/** The currency codes this screen accepts without a lookup: three capital letters. */
export function currencyProblem(raw: string): boolean {
  return !/^[A-Z]{3}$/.test(raw.trim().toUpperCase());
}
