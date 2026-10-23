import { describe, expect, it } from 'vitest';
import { agreementCreate, intercompanyReconciliation, runPeriods } from './groups';
import { COMPANY_HEADER } from './client';

/** A fetch double that records the request and answers with a fixed body. */
function recorder(body: unknown) {
  const calls: { url: string; init: RequestInit }[] = [];
  const fetchFake = (async (url: string | URL | Request, init?: RequestInit) => {
    calls.push({ url: String(url), init: init ?? {} });
    return new Response(JSON.stringify(body), { status: 200, headers: { 'Content-Type': 'application/json' } });
  }) as typeof fetch;
  return { calls, fetch: fetchFake };
}

describe('groups API calls', () => {
  it('posts the periods of one agreement to its run-period path, as the person', async () => {
    const rec = recorder({ agreement_id: 'AG1', periods: [] });
    await runPeriods({ baseUrl: 'http://api.test', fetch: rec.fetch }, 'AG1', ['2026-01']);
    expect(rec.calls[0]?.url).toBe('http://api.test/v1/agreements/AG1/run-period');
    expect(rec.calls[0]?.init.method).toBe('POST');
    expect(rec.calls[0]?.init.credentials).toBe('include');
    expect(JSON.parse(String(rec.calls[0]?.init.body))).toEqual({ periods: ['2026-01'] });
  });

  it('acts for the company a call names, not the page company, when it is given one', async () => {
    const rec = recorder({ group_id: 'G', pairs: [] });
    await intercompanyReconciliation({ baseUrl: 'http://api.test', fetch: rec.fetch, company: 'FZIP01' }, 'G');
    const headers = rec.calls[0]?.init.headers as Record<string, string>;
    expect(headers[COMPANY_HEADER]).toBe('FZIP01');
    expect(rec.calls[0]?.url).toBe('http://api.test/v1/groups/G/reconciliation');
  });

  it('sends a new agreement with both sides accounts and the company ids', async () => {
    const rec = recorder({ licensor: {}, licensee: {} });
    await agreementCreate({ baseUrl: 'http://api.test', fetch: rec.fetch }, {
      group_id: 'G',
      licensor_company_id: 'L',
      licensee_company_id: 'E',
      kind: 'royalty',
      basis: 'pct_of_revenue_bp',
      rate_bp: 500,
      currency: 'EUR',
      licensor: { royalty_account: '4060', ic_account: '1200', bank_account: '1000' },
      licensee: { royalty_account: '6100', ic_account: '2070', wht_account: '2060', bank_account: '1000' },
    });
    const body = JSON.parse(String(rec.calls[0]?.init.body)) as Record<string, unknown>;
    expect(body.licensor_company_id).toBe('L');
    expect(body.rate_bp).toBe(500);
    expect((body.licensee as Record<string, unknown>).wht_account).toBe('2060');
  });
});
