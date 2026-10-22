/**
 * Browser-side session state. The API key lives in sessionStorage only (never
 * localStorage), so closing the tab forgets it. Every storage access is wrapped:
 * private windows and blocked storage must not break the app.
 */
import { createContext } from 'preact';
import { useContext } from 'preact/hooks';
import type { ClientConfig } from './api/client';
import type { Person } from './api/auth';
import { defaultServerUrl } from './api/config';
import { currentPeriod, isPeriod } from './lib/period';
import { normaliseRange, presetRange, type MonthRange } from './lib/trends';
import type { Unit } from './api/types';

/** The server origin the Connect screen starts from: VITE_LEDGERS_API at build time, else the local server. */
export const DEFAULT_SERVER_URL = defaultServerUrl(import.meta.env.VITE_LEDGERS_API);

export interface Credentials {
  baseUrl: string;
  apiKey: string;
}

const KEY_URL = 'ledgers.baseUrl';
const KEY_API = 'ledgers.apiKey';
const KEY_PERIOD = 'ledgers.period';
const KEY_RANGE = 'ledgers.range';
const KEY_CURRENCY = 'ledgers.currency';
const THEME_KEY = 'ledgers.theme';

function readSession(key: string): string | null {
  try {
    return window.sessionStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeSession(key: string, value: string | null): void {
  try {
    if (value === null) window.sessionStorage.removeItem(key);
    else window.sessionStorage.setItem(key, value);
  } catch {
    // Storage unavailable: the session simply lasts for this page view.
  }
}

export function loadCredentials(): Credentials | null {
  const apiKey = readSession(KEY_API);
  if (!apiKey) return null;
  return { baseUrl: readSession(KEY_URL) ?? DEFAULT_SERVER_URL, apiKey };
}

export function saveCredentials(credentials: Credentials): void {
  writeSession(KEY_URL, credentials.baseUrl);
  writeSession(KEY_API, credentials.apiKey);
}

export function clearCredentials(): void {
  writeSession(KEY_API, null);
  writeSession(KEY_URL, null);
  writeSession(KEY_PERIOD, null);
  writeSession(KEY_RANGE, null);
}

export function loadPeriod(): string {
  const stored = readSession(KEY_PERIOD);
  return stored && isPeriod(stored) ? stored : currentPeriod();
}

export function savePeriod(period: string): void {
  writeSession(KEY_PERIOD, period);
}

/** The period range the reports and Overview cover. Defaults to this year to date. */
export function loadRange(now: Date = new Date()): MonthRange {
  const stored = readSession(KEY_RANGE);
  const [from, to] = stored ? stored.split('/') : [];
  if (from && to && isPeriod(from) && isPeriod(to)) return normaliseRange({ from, to });
  return presetRange('thisYear', now);
}

export function saveRange(range: MonthRange): void {
  writeSession(KEY_RANGE, `${range.from}/${range.to}`);
}

export function loadCurrency(): string {
  const stored = readSession(KEY_CURRENCY);
  return stored && /^[A-Z]{3}$/.test(stored) ? stored : 'EUR';
}

export function saveCurrency(currency: string): void {
  writeSession(KEY_CURRENCY, currency);
}

export type Theme = 'dark' | 'light';

export function loadTheme(): Theme {
  try {
    return window.localStorage.getItem(THEME_KEY) === 'light' ? 'light' : 'dark';
  } catch {
    return 'dark';
  }
}

export function saveTheme(theme: Theme): void {
  try {
    window.localStorage.setItem(THEME_KEY, theme);
  } catch {
    // Theme is a convenience; ignore storage failures.
  }
}

export type ToastKind = 'ok' | 'error';

export interface Session {
  client: ClientConfig;
  /** The month the single-month pages (journal, new entry, statements) use: the end of the range. */
  period: string;
  setPeriod: (period: string) => void;
  /** The period the Overview and the period picker cover. */
  range: MonthRange;
  setRange: (range: MonthRange) => void;
  currency: string;
  setCurrency: (currency: string) => void;
  online: boolean;
  toast: (message: string, kind?: ToastKind) => void;
  signOut: () => void;
  /** The signed-in person; absent for an agent key. */
  person?: Person;
  /** Approvals waiting for the person; null when unknown or not offered (agent key). */
  approvalCount: number | null;
  /** Re-reads the approvals count (after a decision). */
  refreshApprovals: () => void;
  /** The unit scope every screen reads: a unit code from the URL (`?unit=`), or null for the whole company. */
  unit: string | null;
  /** The active business units, in code order. Empty when the company has none, which hides the switcher. */
  units: Unit[];
  /** Re-reads the business units (after a unit is added, renamed or archived). */
  refreshUnits: () => void;
}

export const SessionContext = createContext<Session | null>(null);

export function useSession(): Session {
  const value = useContext(SessionContext);
  if (!value) throw new Error('useSession outside SessionContext');
  return value;
}
