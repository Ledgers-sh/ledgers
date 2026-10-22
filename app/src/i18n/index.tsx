/**
 * The app's translations: a typed message catalogue per language, the
 * language in use (remembered on this device), and locale-aware formatting.
 *
 * Messages come from catalogues that TypeScript checks against en.ts, so a
 * key cannot go missing from one language. Formatting goes through Intl with
 * the reader's language, and the document's direction follows the language
 * (Arabic is right-to-left; amounts inside it stay left-to-right).
 */
import type { ComponentChildren } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { ar } from './ar';
import { de } from './de';
import { en, type MessageKey } from './en';

export type { MessageKey } from './en';
import { id } from './id';
import { accountNameFromCatalogue } from './chart';

export type LocaleCode = 'en' | 'de' | 'id' | 'ar';

export interface LocaleInfo {
  code: LocaleCode;
  /** The language's own name, shown in the picker whatever the current language is. */
  name: string;
  dir: 'ltr' | 'rtl';
  /** The Intl locale used for dates and numbers. Arabic keeps Western digits, the form accounts are written in. */
  intl: string;
}

export const LOCALES: readonly LocaleInfo[] = [
  { code: 'en', name: 'English', dir: 'ltr', intl: 'en-GB' },
  { code: 'de', name: 'Deutsch', dir: 'ltr', intl: 'de-DE' },
  { code: 'id', name: 'Bahasa Indonesia', dir: 'ltr', intl: 'id-ID' },
  { code: 'ar', name: 'العربية', dir: 'rtl', intl: 'ar-u-nu-latn' },
];

const CATALOGUES: Record<LocaleCode, Record<MessageKey, string>> = { en, de, id, ar };

const STORAGE_KEY = 'ledgers.locale';

function isLocale(value: unknown): value is LocaleCode {
  return typeof value === 'string' && LOCALES.some((locale) => locale.code === value);
}

/** The language to start with: the saved choice, else the browser's language when the app has it, else English. */
export function initialLocale(): LocaleCode {
  try {
    const saved = window.localStorage.getItem(STORAGE_KEY);
    if (isLocale(saved)) return saved;
  } catch {
    // Storage blocked: fall through to the browser language.
  }
  const browser = typeof navigator !== 'undefined' ? navigator.language.slice(0, 2).toLowerCase() : 'en';
  return isLocale(browser) ? browser : 'en';
}

let current: LocaleCode = 'en';
const listeners = new Set<() => void>();

/** The language in use now. */
export function currentLocale(): LocaleInfo {
  return LOCALES.find((locale) => locale.code === current) ?? LOCALES[0];
}

/** Switches the language: remembers the choice, sets the document's language and direction, and re-renders listeners. */
export function setLocale(code: LocaleCode): void {
  current = code;
  applyDocumentLocale();
  try {
    window.localStorage.setItem(STORAGE_KEY, code);
  } catch {
    // The choice lasts for this page view only.
  }
  listeners.forEach((listener) => listener());
}

/** Sets <html lang> and <html dir> for the language in use. */
export function applyDocumentLocale(): void {
  if (typeof document === 'undefined') return;
  const info = currentLocale();
  document.documentElement.lang = info.code;
  document.documentElement.dir = info.dir;
}

/** Re-renders the calling component whenever the language changes. Returns the language in use. */
export function useLocale(): LocaleInfo {
  const [, setTick] = useState(0);
  useEffect(() => {
    const listener = () => setTick((tick) => tick + 1);
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  }, []);
  return currentLocale();
}

/** Replaces {name} placeholders. A missing value keeps its placeholder, so a bad call is visible, not silent. */
function fill(template: string, vars: Record<string, string | number> | undefined): string {
  if (!vars) return template;
  return template.replace(/\{(\w+)\}/g, (match, name: string) => (name in vars ? String(vars[name]) : match));
}

/** The message in the language in use, with placeholders filled. */
export function t(key: MessageKey, vars?: Record<string, string | number>): string {
  return fill(CATALOGUES[current][key], vars);
}

/**
 * The message with some placeholders replaced by elements (a code span, a
 * strong word). The text around them stays translatable as one string.
 */
export function tRich(key: MessageKey, parts: Record<string, ComponentChildren>): ComponentChildren[] {
  const template = CATALOGUES[current][key];
  const out: ComponentChildren[] = [];
  let rest = template;
  const pattern = /\{(\w+)\}/;
  for (let match = pattern.exec(rest); match; match = pattern.exec(rest)) {
    const [token, name] = match;
    const before = rest.slice(0, match.index);
    if (before) out.push(before);
    out.push(name in parts ? parts[name] : token);
    rest = rest.slice(match.index + token.length);
  }
  if (rest) out.push(rest);
  return out;
}

/**
 * The name of an account from its name key. The catalogue built from the
 * templates wins; a key it does not know reads as a humanised fallback: the
 * last segment with separators turned into spaces ("account.cash_at_bank"
 * becomes "Cash at bank").
 */
export function accountName(nameKey: string): string {
  const translated = accountNameFromCatalogue(nameKey, current);
  if (translated) return translated;
  const segment = nameKey.split('.').pop() ?? nameKey;
  const bare = segment.replace(/[._-]+/g, ' ').trim();
  return bare.charAt(0).toUpperCase() + bare.slice(1);
}

/** Intl locale for the language in use. */
export function intlLocale(): string {
  return currentLocale().intl;
}

/** A date such as 2026-10-03 in the reader's format. Invalid input is returned unchanged. */
export function formatDate(iso: string): string {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(iso);
  if (!match) return iso;
  const date = new Date(Date.UTC(Number(match[1]), Number(match[2]) - 1, Number(match[3])));
  return new Intl.DateTimeFormat(intlLocale(), { year: 'numeric', month: 'short', day: 'numeric', timeZone: 'UTC' }).format(date);
}

/** A YYYY-MM period as a month and year, such as "Oct 2026". */
export function formatPeriod(period: string): string {
  const match = /^(\d{4})-(\d{2})$/.exec(period);
  if (!match) return period;
  const date = new Date(Date.UTC(Number(match[1]), Number(match[2]) - 1, 1));
  return new Intl.DateTimeFormat(intlLocale(), { year: 'numeric', month: 'short', timeZone: 'UTC' }).format(date);
}

/** Starts in the saved language, or the browser's when the app has it. Call once, before the first render. */
export function restoreLocale(): void {
  current = initialLocale();
  applyDocumentLocale();
}

/**
 * A message with a count. The form is picked by the language's plural rules;
 * a language with one form (Indonesian) uses the "other" entry.
 */
export function tn(base: string, count: number, vars?: Record<string, string | number>): string {
  const form = new Intl.PluralRules(intlLocale()).select(count) === 'one' ? 'one' : 'other';
  return t(`${base}.${form}` as MessageKey, { count, ...vars });
}
