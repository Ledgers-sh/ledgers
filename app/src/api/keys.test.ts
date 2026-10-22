import { describe, expect, it } from 'vitest';
import { keysCreate, keysList, keysRevoke } from './commands';

interface Captured {
  url: string;
  init: RequestInit;
}

function fakeFetch(status: number, body: unknown) {
  const captured: Captured[] = [];
  const fetchImpl = (async (input: RequestInfo | URL, init?: RequestInit) => {
    captured.push({ url: String(input), init: init ?? {} });
    return new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } });
  }) as typeof fetch;
  return { fetchImpl, captured };
}

describe('agent key commands', () => {
  it('lists keys with the session cookie and no bearer key', async () => {
    const { fetchImpl, captured } = fakeFetch(200, { keys: [] });
    await keysList({ baseUrl: 'http://127.0.0.1:3000/', fetch: fetchImpl });
    expect(captured[0].url).toBe('http://127.0.0.1:3000/v1/keys');
    expect(captured[0].init.method).toBe('GET');
    expect(captured[0].init.credentials).toBe('include');
    expect(new Headers(captured[0].init.headers as HeadersInit).get('Authorization')).toBeNull();
  });

  it('creates a key with its name, tier and limits, and returns the token', async () => {
    const { fetchImpl, captured } = fakeFetch(200, { key: { id: 'k1' }, token: 'lk_live_x_y' });
    const out = await keysCreate(
      { baseUrl: 'http://127.0.0.1:3000', fetch: fetchImpl },
      { name: 'Bot', tier: 'read', limits: { EUR: { per_entry: '1.00', per_day: '2.00' } } },
    );
    expect(out.token).toBe('lk_live_x_y');
    expect(captured[0].url).toBe('http://127.0.0.1:3000/v1/keys');
    expect(captured[0].init.method).toBe('POST');
    expect(JSON.parse(String(captured[0].init.body))).toEqual({
      name: 'Bot',
      tier: 'read',
      limits: { EUR: { per_entry: '1.00', per_day: '2.00' } },
    });
  });

  it('revokes by id, encoding it in the path', async () => {
    const { fetchImpl, captured } = fakeFetch(200, { key: { id: 'a/b' } });
    await keysRevoke({ baseUrl: 'http://127.0.0.1:3000', fetch: fetchImpl }, 'a/b');
    expect(captured[0].url).toBe('http://127.0.0.1:3000/v1/keys/a%2Fb/revoke');
    expect(captured[0].init.method).toBe('POST');
  });
});
