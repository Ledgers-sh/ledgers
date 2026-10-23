/**
 * Companies in the app: the one a request acts for (the URL's `?company=`),
 * the role a person holds in each, and the rules for the create dialog. The
 * server holds the same rules; these checks answer the person before a request
 * is sent.
 */

/** The query parameter that names the company. Absent means the deployment's own company. */
export const COMPANY_PARAM = 'company';

/** A company id as the server issues it: letters and digits, at most 64 characters. */
export const COMPANY_ID = /^[A-Za-z0-9]{1,64}$/;

/** The longest company name the server takes, in characters. */
export const COMPANY_NAME_MAX = 200;

/** The company named by a location's query string, or null when none is named or the id is malformed. */
export function companyFromSearch(search: string): string | null {
  const value = new URLSearchParams(search).get(COMPANY_PARAM)?.trim() ?? '';
  return COMPANY_ID.test(value) ? value : null;
}

/**
 * The same query string with the company set to `id`, or removed when `id` is
 * null. Every other parameter is kept, so the unit and period are not touched
 * here: the caller decides whether they reset.
 */
export function searchWithCompany(search: string, id: string | null): string {
  const params = new URLSearchParams(search);
  if (id === null) params.delete(COMPANY_PARAM);
  else params.set(COMPANY_PARAM, id);
  const text = params.toString();
  return text === '' ? '' : `?${text}`;
}

/**
 * The address of the app with the company switched to `id`: the same page, the
 * company in the query, and the overview route with no unit scope. The unit
 * and period are the defaults again, so nothing of the old company is carried.
 */
export function companyHref(pathname: string, id: string, hash = '#/overview'): string {
  return `${pathname}${searchWithCompany('', id)}${hash}`;
}

/** The roles a company membership can carry, as the server names them. */
export const COMPANY_ROLES = ['owner', 'admin', 'accountant', 'approver', 'viewer', 'auditor'] as const;

export type CompanyRole = (typeof COMPANY_ROLES)[number];

/** The CSS class of a role's badge (the same badges the People screen uses). */
export function roleBadgeClass(role: string): string {
  return COMPANY_ROLES.includes(role as CompanyRole) ? `role-badge role-${role}` : 'role-badge';
}

/** A company name must be 1 to 200 characters once trimmed. */
export function companyNameProblem(name: string): 'empty' | 'too-long' | null {
  const trimmed = name.trim();
  if (trimmed === '') return 'empty';
  if (Array.from(trimmed).length > COMPANY_NAME_MAX) return 'too-long';
  return null;
}

/** A base currency as the server takes it: three uppercase ISO 4217 letters. */
export function currencyProblem(code: string): boolean {
  return !/^[A-Z]{3}$/.test(code);
}

/** The fiscal year's first month, 1 to 12, as the server takes it. */
export function fiscalYearStartProblem(month: number): boolean {
  return !Number.isInteger(month) || month < 1 || month > 12;
}
