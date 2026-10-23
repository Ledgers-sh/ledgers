/**
 * Company calls: the companies a signed-in person belongs to, the country packs
 * a new company can start from, and creating a company. These are person calls:
 * the server refuses an agent key, which is not a person.
 */
import { request, type ClientConfig } from './client';

/** One company the person belongs to, with the role they hold in it. */
export interface CompanySummary {
  id: string;
  name: string;
  baseCurrency: string;
  role: string;
  /** The company's logo document (its own documents hold it), or null. */
  logo_document_id?: string | null;
}

/** A bundled country pack: the defaults a new company starts from. */
export interface CountryPackSummary {
  id: string;
  name: string;
  iso: string;
  /** The pack's currency, or null for the generic pack (the company chooses). */
  currency: string | null;
  fiscalYearStartMonth: number;
}

export interface CreateCompanyInput {
  name: string;
  baseCurrency: string;
  countryPack: string;
  fiscalYearStartMonth: number;
}

export async function companiesList(client: ClientConfig): Promise<CompanySummary[]> {
  const body = await request<{ companies: CompanySummary[] }>(client, { method: 'GET', path: '/v1/companies' });
  return body.companies;
}

export async function countryPacksList(client: ClientConfig): Promise<CountryPackSummary[]> {
  const body = await request<{ packs: CountryPackSummary[] }>(client, { method: 'GET', path: '/v1/companies/packs' });
  return body.packs;
}

export function companyCreate(client: ClientConfig, input: CreateCompanyInput): Promise<CompanySummary> {
  return request<CompanySummary>(client, { method: 'POST', path: '/v1/companies', body: input });
}
