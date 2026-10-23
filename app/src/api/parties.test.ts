import { describe, expect, it } from 'vitest';
import { partiesList, partyMerge, partySetBank, partyValidate } from './parties';
import { ApiError } from './client';

interface Captured {
  url: string;
  method: string;
  body: unknown;
}

function fakeFetch(status: number, body: unknown = {}) {
  const captured: Captured[] = [];
  const fetchImpl = (async (input: RequestInfo | URL, init?: RequestInit) => {
    captured.push({ url: String(input), method: init?.method ?? 'GET', body: init?.body === undefined ? undefined : JSON.parse(String(init.body)) });
    return new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } });
  }) as typeof fetch;
  return { captured, config: { baseUrl: 'http://api.test', fetch: fetchImpl } };
}

describe('party calls', () => {
  it('sends the list filters as a query, leaving out the ones not set', async () => {
    const { captured, config } = fakeFetch(200, { parties: [], total: 0 });
    await partiesList(config, { limit: 50, offset: 50, q: 'acme', roles: 'customer', needs_review: false });
    expect(captured[0].method).toBe('GET');
    const url = new URL(captured[0].url);
    expect(url.pathname).toBe('/v1/parties');
    expect(url.searchParams.get('limit')).toBe('50');
    expect(url.searchParams.get('offset')).toBe('50');
    expect(url.searchParams.get('q')).toBe('acme');
    expect(url.searchParams.get('roles')).toBe('customer');
    expect(url.searchParams.get('needs_review')).toBe('false');
    await partiesList(config, {});
    expect(new URL(captured[1].url).search).toBe('');
  });

  it('posts a validation to /validate with the fields it names', async () => {
    const { captured, config } = fakeFetch(200, { valid: false, errors: [{ field: 'iban', message: 'wrong' }] });
    const result = await partyValidate(config, { iban: 'DE00', bic: 'X' });
    expect(captured[0]).toMatchObject({ method: 'POST', body: { iban: 'DE00', bic: 'X' } });
    expect(new URL(captured[0].url).pathname).toBe('/v1/parties/validate');
    expect(result.errors[0].field).toBe('iban');
  });

  it('sends a bank change to set-bank and a merge with the source as from', async () => {
    const { captured, config } = fakeFetch(200, { party: {} });
    await partySetBank(config, 'p 1', { iban: 'DE89370400440532013000', bic: null, reason: 'called the contact' });
    expect(new URL(captured[0].url).pathname).toBe('/v1/parties/p%201/set-bank');
    expect(captured[0].body).toEqual({ iban: 'DE89370400440532013000', bic: null, reason: 'called the contact' });
    await partyMerge(config, 'survivor', 'source');
    expect(new URL(captured[1].url).pathname).toBe('/v1/parties/survivor/merge');
    expect(captured[1].body).toEqual({ from: 'source' });
  });

  it('surfaces a refusal as an ApiError carrying the server detail', async () => {
    const { config } = fakeFetch(422, { detail: 'source party is already merged' });
    await expect(partyMerge(config, 'a', 'b')).rejects.toBeInstanceOf(ApiError);
  });
});
