/**
 * Business units in the app: the scope in the URL (`?unit=LDG`), the colour
 * and code a unit is shown with, and the rules a line or bill must meet to
 * carry one. The server holds the same rules; these checks answer the person
 * before a request is sent.
 */
import type { Account, Unit } from '../api/types';

/** The query parameter that names the unit scope. Absent means the whole company (consolidated). */
export const UNIT_PARAM = 'unit';

/** Colour a unit gets when none is chosen (the agent blue of the palette). */
export const DEFAULT_UNIT_COLOUR = '#7FA6C9';

/** A unit code as the server takes it: 2 to 16 letters or digits. */
export const UNIT_CODE = /^[A-Za-z0-9]{2,16}$/;

/** `#RRGGBB`, the only colour form the server stores. */
export const UNIT_COLOUR = /^#[0-9A-Fa-f]{6}$/;

/** The route part of a location hash without its query, e.g. "journal/abc" for "journal/abc?unit=LDG". */
export function routeOf(hash: string): string {
  return hash.replace(/\?.*$/, '');
}

/**
 * The unit scope named by a location hash (without the leading "#/"). Returns
 * the upper-cased code, or null for the whole company (no parameter, an empty
 * one, or `all`). A malformed code is ignored rather than sent to the server.
 */
export function unitFromHash(hash: string): string | null {
  const query = hash.indexOf('?');
  if (query < 0) return null;
  const value = new URLSearchParams(hash.slice(query + 1)).get(UNIT_PARAM);
  if (value === null) return null;
  const code = value.trim();
  if (code === '' || code.toLowerCase() === 'all' || !UNIT_CODE.test(code)) return null;
  return code.toUpperCase();
}

/**
 * The same location with its unit scope set to `code`, or with the scope
 * removed when `code` is null. Every other parameter and the route are kept.
 */
export function hashWithUnit(hash: string, code: string | null): string {
  const query = hash.indexOf('?');
  const route = query < 0 ? hash : hash.slice(0, query);
  const params = new URLSearchParams(query < 0 ? '' : hash.slice(query + 1));
  if (code === null) params.delete(UNIT_PARAM);
  else params.set(UNIT_PARAM, code);
  const text = params.toString();
  return text === '' ? route : `${route}?${text}`;
}

/** The unit parameter an API call sends for a scope: the code, or undefined for the whole company. */
export function unitQuery(scope: string | null | undefined): string | undefined {
  return scope ? scope : undefined;
}

/** Units that can be chosen for new postings: not archived, in code order. */
export function activeUnits(units: readonly Unit[]): Unit[] {
  return units.filter((unit) => !unit.archived).sort((a, b) => a.code.localeCompare(b.code));
}

/** The colour to show a unit with: its own when it is a `#RRGGBB` value, else the default. */
export function colourOf(unit: { colour?: string | null } | null | undefined): string {
  return unit?.colour && UNIT_COLOUR.test(unit.colour) ? unit.colour : DEFAULT_UNIT_COLOUR;
}

/**
 * Whether an account's lines must name a unit once the company has units:
 * income and expense lines, and lines on the inter-unit accounts (tags
 * `inter-unit-due-from`, `inter-unit-due-to`, `inter-unit-clearing`). The
 * balance sheet's other accounts may stay company-level.
 */
export function accountNeedsUnit(account: Pick<Account, 'account_type' | 'tags'> | undefined): boolean {
  if (!account) return false;
  if (account.account_type === 'income' || account.account_type === 'expense') return true;
  return account.tags.some((tag) => tag.startsWith('inter-unit-'));
}

/** A share of a total in basis points (0 to 10000), zero when the total is zero. */
export function shareBps(part: bigint, total: bigint): number {
  if (total === 0n) return 0;
  const scaled = (part * 10_000n) / total;
  return Number(scaled);
}

/** Sets the unit scope of the current screen, keeping the route and its other parameters. */
export function navigateToUnit(code: string | null): void {
  const current = window.location.hash.replace(/^#\/?/, '');
  window.location.hash = `#/${hashWithUnit(current, code)}`;
}
