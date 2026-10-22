import { afterEach, describe, expect, it } from 'vitest';
import { COMPANY_HEADER, getActiveCompany, request, setActiveCompany, type ClientConfig } from './client';
import { accountsList, entriesList, reportsTrialBalance } from './commands';
import { companiesList, companyCreate, countryPacksList } from './companies';
import { companyGet } from './settings';

interface Captured {
  url: string;
  init: RequestInit;
}

/** A fetch that records every call and answers 200 with `body`. */
function recordingFetch(body: unknown = { accounts: [], companies: [], packs: [] }) {
  const captured: Captured[] = [];
  const fetchImpl = (async (input: RequestInfo | URL, init?: RequestInit) => {
    captured.push({ url: String(input), init: init ?? {} });
    return new Response(JSON.stringify(body), { status: 200, headers: { 'Content-Type': 'application/json' } });
  }) as typeof fetch;
  return { fetchImpl, captured };
}

const headerOf = (init: RequestInit) => new Headers(init.headers as HeadersInit).get(COMPANY_HEADER);

afterEach(() => setActiveCompany(null));

describe('the company header', () => {
  it('is sent with every request once a company is active, and with a key or a session alike', async () => {
    setActiveCompany('HALDEN01');
    const person = recordingFetch();
    const key = recordingFetch({ accounts: [], companies: [], packs: [] });
    const personConfig: ClientConfig = { baseUrl: 'http://127.0.0.1:3000', fetch: person.fetchImpl };
    const keyConfig: ClientConfig = { baseUrl: 'http://127.0.0.1:3000', apiKey: 'test_sk_key', fetch: key.fetchImpl };

    await companiesList(personConfig);
    await countryPacksList(personConfig);
    await companyCreate(personConfig, { name: 'Acme', baseCurrency: 'EUR', countryPack: 'generic', fiscalYearStartMonth: 1 });
    await accountsList(keyConfig);
    await entriesList(keyConfig, {});
    await reportsTrialBalance(keyConfig, '2026-10');
    await companyGet(keyConfig);

    const calls = [...person.captured, ...key.captured];
    expect(calls).toHaveLength(7);
    for (const call of calls) {
      expect(headerOf(call.init), call.url).toBe('HALDEN01');
    }
  });

  it('is not sent when no company is named: the deployment company answers', async () => {
    const { fetchImpl, captured } = recordingFetch();
    await companiesList({ baseUrl: 'http://127.0.0.1:3000', fetch: fetchImpl });
    expect(headerOf(captured[0].init)).toBeNull();
    expect(getActiveCompany()).toBeNull();
  });

  it('changes with the active company, so a switch sends the new one at once', async () => {
    const { fetchImpl, captured } = recordingFetch();
    const config: ClientConfig = { baseUrl: 'http://127.0.0.1:3000', fetch: fetchImpl };
    setActiveCompany('AAA111');
    await request(config, { method: 'GET', path: '/v1/companies' });
    setActiveCompany('BBB222');
    await request(config, { method: 'GET', path: '/v1/companies' });
    expect(headerOf(captured[0].init)).toBe('AAA111');
    expect(headerOf(captured[1].init)).toBe('BBB222');
  });
});
