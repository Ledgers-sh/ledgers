import { describe, expect, it } from 'vitest';
import { ApiError, buildUrl, kindForStatus } from './client';
import {
  accountsList,
  entriesGet,
  entriesList,
  entriesPost,
  entriesReverse,
  reportsBalanceSheet,
  reportsProfitAndLoss,
  reportsTrialBalance,
  reportsAccountBalanceByDimension,
} from './commands';
import type { PostEntryInput } from './types';

interface Captured {
  url: string;
  init: RequestInit;
}

function fakeFetch(status: number, body: unknown = {}, bodyText?: string) {
  const captured: Captured[] = [];
  const fetchImpl = (async (input: RequestInfo | URL, init?: RequestInit) => {
    captured.push({ url: String(input), init: init ?? {} });
    const text = bodyText ?? JSON.stringify(body);
    return new Response(text, { status, headers: { 'Content-Type': 'application/json' } });
  }) as typeof fetch;
  return { fetchImpl, captured };
}

const config = (fetchImpl: typeof fetch) => ({ baseUrl: 'http://127.0.0.1:3000/', apiKey: 'test_sk_key', fetch: fetchImpl });

const headersOf = (init: RequestInit) => new Headers(init.headers as HeadersInit);

describe('request building', () => {
  it('sends bearer auth to the trailing-slash-normalised base URL', async () => {
    const { fetchImpl, captured } = fakeFetch(200, { accounts: [] });
    await accountsList(config(fetchImpl));
    expect(captured[0].url).toBe('http://127.0.0.1:3000/v1/accounts');
    expect(captured[0].init.method).toBe('GET');
    expect(headersOf(captured[0].init).get('Authorization')).toBe('Bearer test_sk_key');
    expect(captured[0].init.body).toBeUndefined();
  });

  it('posts entries.post with the PostEntryInput body as JSON', async () => {
    const { fetchImpl, captured } = fakeFetch(200, {
      entry: { entry_id: 'e1', journal_id: 'general', fiscal_year: 2026, number: 1, recorded_at: 'now' },
    });
    const input: PostEntryInput = {
      journal_id: 'general',
      fiscal_year: 2026,
      entry: {
        journal_id: 'general',
        document_date: '2026-10-01',
        posting_date: '2026-10-01',
        tax_point_date: null,
        base_currency: 'EUR',
        description: 'Rent',
        source_documents: [],
        external_id: 'app-1',
        source_app: 'ledgers-app',
        reverses: null,
        lines: [],
      },
    };
    const result = await entriesPost(config(fetchImpl), input);
    expect(result.entry.number).toBe(1);
    expect(captured[0].url).toBe('http://127.0.0.1:3000/v1/entries');
    expect(captured[0].init.method).toBe('POST');
    expect(headersOf(captured[0].init).get('Content-Type')).toBe('application/json');
    expect(JSON.parse(String(captured[0].init.body))).toEqual(input);
  });

  it('reverses by external id, encoding the id in the path', async () => {
    const { fetchImpl, captured } = fakeFetch(200, { entry: {} });
    await entriesReverse(config(fetchImpl), 'a/b c', { posting_date: '2026-10-02', description: 'Reversal' });
    expect(captured[0].url).toBe('http://127.0.0.1:3000/v1/entries/a%2Fb%20c/reverse');
    expect(captured[0].init.method).toBe('POST');
    expect(JSON.parse(String(captured[0].init.body))).toEqual({ posting_date: '2026-10-02', description: 'Reversal' });
  });

  it('lists and gets entries', async () => {
    const { fetchImpl, captured } = fakeFetch(200, { entries: [] });
    await entriesList(config(fetchImpl));
    expect(captured[0].url).toBe('http://127.0.0.1:3000/v1/entries');
    await entriesGet(config(fetchImpl), 'ext-9');
    expect(captured[1].url).toBe('http://127.0.0.1:3000/v1/entries/ext-9');
  });

  it('sends report periods as query parameters', async () => {
    const { fetchImpl, captured } = fakeFetch(200, {});
    await reportsTrialBalance(config(fetchImpl), '2026-10');
    expect(captured[0].url).toBe('http://127.0.0.1:3000/v1/reports/trial-balance?period=2026-10');
    await reportsProfitAndLoss(config(fetchImpl), '2026-10');
    expect(captured[1].url).toBe('http://127.0.0.1:3000/v1/reports/p-and-l?period=2026-10');
    await reportsBalanceSheet(config(fetchImpl), '2026-10-31');
    expect(captured[2].url).toBe('http://127.0.0.1:3000/v1/reports/balance-sheet?as_of=2026-10-31');
    await reportsAccountBalanceByDimension(config(fetchImpl), { account: '6000', dimension: 'cost-centre', period: '2026-10' });
    expect(captured[3].url).toBe(
      'http://127.0.0.1:3000/v1/reports/account-balance-by-dimension?account=6000&dimension=cost-centre&period=2026-10',
    );
  });

  it('buildUrl skips empty query values', () => {
    expect(buildUrl('http://x', '/v1/a', { period: '', as_of: undefined, q: 'y' })).toBe('http://x/v1/a?q=y');
  });
});

describe('error mapping', () => {
  it('maps status codes to kinds', () => {
    expect(kindForStatus(401)).toBe('unauthorized');
    expect(kindForStatus(403)).toBe('forbidden');
    expect(kindForStatus(422)).toBe('validation');
    expect(kindForStatus(400)).toBe('validation');
    expect(kindForStatus(404)).toBe('not_found');
    expect(kindForStatus(501)).toBe('unavailable');
    expect(kindForStatus(500)).toBe('server');
  });

  it('maps 401 to unauthorized with a readable message', async () => {
    const { fetchImpl } = fakeFetch(401, { title: 'Unauthorized' });
    const error = await accountsList(config(fetchImpl)).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ApiError);
    expect((error as ApiError).kind).toBe('unauthorized');
    expect((error as ApiError).status).toBe(401);
    expect((error as ApiError).message).toMatch(/API key was rejected/);
  });

  it('maps 403 to forbidden and keeps the server detail', async () => {
    const { fetchImpl } = fakeFetch(403, { title: 'Forbidden', detail: 'tier draft cannot post' });
    const error = (await entriesPost(config(fetchImpl), {} as PostEntryInput).catch((e: unknown) => e)) as ApiError;
    expect(error.kind).toBe('forbidden');
    expect(error.detail).toBe('tier draft cannot post');
  });

  it('maps 422 to validation and surfaces the problem detail', async () => {
    const { fetchImpl } = fakeFetch(422, { title: 'Validation failed', detail: 'entry does not balance' });
    const error = (await entriesPost(config(fetchImpl), {} as PostEntryInput).catch((e: unknown) => e)) as ApiError;
    expect(error.kind).toBe('validation');
    expect(error.status).toBe(422);
    expect(error.message).toBe('entry does not balance');
  });

  it('maps a non-JSON error body to the generic message', async () => {
    const { fetchImpl } = fakeFetch(500, undefined, '<html>boom</html>');
    const error = (await accountsList(config(fetchImpl)).catch((e: unknown) => e)) as ApiError;
    expect(error.kind).toBe('server');
    expect(error.detail).toBeNull();
  });

  it('maps a failed fetch to network', async () => {
    const failing = (async () => {
      throw new TypeError('Failed to fetch');
    }) as typeof fetch;
    const error = (await accountsList(config(failing)).catch((e: unknown) => e)) as ApiError;
    expect(error.kind).toBe('network');
    expect(error.status).toBeNull();
  });
});

describe('person sessions', () => {
  it('sends the session cookie and no bearer header when there is no key', async () => {
    const { fetchImpl, captured } = fakeFetch(200, { accounts: [] });
    await accountsList({ baseUrl: 'http://127.0.0.1:3000', fetch: fetchImpl });
    expect(captured[0].init.credentials).toBe('include');
    expect(headersOf(captured[0].init).get('Authorization')).toBeNull();
  });

  it('keeps bearer auth for an agent key and does not send the cookie', async () => {
    const { fetchImpl, captured } = fakeFetch(200, { accounts: [] });
    await accountsList(config(fetchImpl));
    expect(captured[0].init.credentials).toBeUndefined();
    expect(headersOf(captured[0].init).get('Authorization')).toBe('Bearer test_sk_key');
  });
});
