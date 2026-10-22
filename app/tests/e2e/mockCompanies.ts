/**
 * The companies a signed-in person belongs to, against the fake API: the list
 * the switcher reads, the country packs the create dialog offers, and creating
 * a company. A company is created with the person as its owner. The fake holds
 * three companies at most per person, so a fourth is refused as the server does.
 */

export interface MockCompany {
  id: string;
  name: string;
  baseCurrency: string;
  role: string;
}

interface Reply {
  status: number;
  body: unknown;
  problem?: boolean;
}

/** The most companies one person may hold in this fake. */
export const MOCK_COMPANY_LIMIT = 3;

export const HALDEN: MockCompany = { id: 'HALDEN01', name: 'Halden Works GmbH', baseCurrency: 'EUR', role: 'owner' };
export const FACTORY: MockCompany = { id: 'FZDEMO02', name: 'Factory Zero (demo)', baseCurrency: 'EUR', role: 'viewer' };

export const companyState: { companies: MockCompany[] } = { companies: [] };

const PACKS = [
  { id: 'generic', name: 'Generic (company sets its rates)', iso: 'XX', currency: null, fiscalYearStartMonth: 1 },
  { id: 'de', name: 'Germany', iso: 'DE', currency: 'EUR', fiscalYearStartMonth: 1 },
  { id: 'sg', name: 'Singapore', iso: 'SG', currency: 'SGD', fiscalYearStartMonth: 1 },
];

/** Seeds the companies: two (the switcher shows), one, or none (the list is empty). */
export function resetCompanies(seed: 'none' | 'one' | 'many' = 'none'): void {
  companyState.companies = seed === 'many' ? [{ ...HALDEN }, { ...FACTORY }] : seed === 'one' ? [{ ...HALDEN }] : [];
}

function reply(status: number, body: unknown): Reply {
  return { status, body };
}

function problem(status: number, slug: string, title: string, detail: string): Reply {
  return { status, problem: true, body: { type: `https://ledgers.sh/problems/${slug}`, title, status, detail } };
}

/** The companies routes, or null for any other path. */
export function handleCompanies(method: string, url: URL, bodyText: string): Reply | null {
  const path = url.pathname;
  if (method === 'GET' && path === '/v1/companies') return reply(200, { companies: companyState.companies });
  if (method === 'GET' && path === '/v1/companies/packs') return reply(200, { packs: PACKS });
  if (method === 'POST' && path === '/v1/companies') {
    if (companyState.companies.length >= MOCK_COMPANY_LIMIT) {
      return problem(
        429,
        'company-limit-reached',
        'Company limit reached',
        `This person has already created ${companyState.companies.length} companies, the most one person may create here.`,
      );
    }
    const input = JSON.parse(bodyText || '{}') as { name?: string; baseCurrency?: string };
    const created: MockCompany = {
      id: `NEW${companyState.companies.length + 1}X`,
      name: (input.name ?? '').trim(),
      baseCurrency: input.baseCurrency ?? 'EUR',
      role: 'owner',
    };
    companyState.companies.push(created);
    return reply(201, created);
  }
  return null;
}
