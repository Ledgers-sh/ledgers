import { describe, expect, it } from 'vitest';
import { ApiError } from './client';
import { fetchSession, initialOf, signInUrl, signOut, type Person } from './auth';

interface Captured {
  url: string;
  init: RequestInit;
}

function fakeFetch(status: number, body: unknown = {}) {
  const captured: Captured[] = [];
  const fetchImpl = (async (input: RequestInfo | URL, init?: RequestInit) => {
    captured.push({ url: String(input), init: init ?? {} });
    const text = typeof body === 'string' ? body : JSON.stringify(body);
    return new Response(text, { status, headers: { 'Content-Type': 'application/json' } });
  }) as typeof fetch;
  return { fetchImpl, captured };
}

const person: Person = {
  personId: 'person-42',
  email: 'owner@halden.test',
  companyId: 'co_halden',
  role: 'owner',
  roles: ['owner'],
  csrfToken: 'csrf',
  expiresAt: '2026-10-22T10:30:00Z',
};

describe('sign-in calls', () => {
  it('starts the passkey sign-in on the server', () => {
    expect(signInUrl('https://api-staging.ledgers.sh/')).toBe('https://api-staging.ledgers.sh/v1/auth/login');
  });

  it('reads the signed-in person with the cookie and no bearer key', async () => {
    const { fetchImpl, captured } = fakeFetch(200, person);
    const got = await fetchSession('http://127.0.0.1:3000', fetchImpl);
    expect(got).toEqual(person);
    expect(captured[0].url).toBe('http://127.0.0.1:3000/v1/auth/session');
    expect(captured[0].init.credentials).toBe('include');
    expect(new Headers(captured[0].init.headers as HeadersInit).get('Authorization')).toBeNull();
  });

  it('resolves to null when there is no session', async () => {
    const { fetchImpl } = fakeFetch(401, { title: 'Sign-in required' });
    expect(await fetchSession('http://127.0.0.1:3000', fetchImpl)).toBeNull();
  });

  it('rejects a signed-in account that may not use the company, with the server detail', async () => {
    const { fetchImpl } = fakeFetch(403, { detail: 'a@b.test is signed in, but only the owner can use this ledger.' });
    const error = await fetchSession('http://127.0.0.1:3000', fetchImpl).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ApiError);
    expect((error as ApiError).kind).toBe('forbidden');
    expect((error as ApiError).detail).toContain('only the owner');
  });

  it('maps an unreachable server to a network error', async () => {
    const failing = (async () => {
      throw new TypeError('Failed to fetch');
    }) as typeof fetch;
    const error = await fetchSession('http://127.0.0.1:3000', failing).catch((e: unknown) => e);
    expect((error as ApiError).kind).toBe('network');
  });

  it('posts the sign-out with the cookie, and never throws', async () => {
    const { fetchImpl, captured } = fakeFetch(204, '');
    await signOut('http://127.0.0.1:3000', fetchImpl);
    expect(captured[0].url).toBe('http://127.0.0.1:3000/v1/auth/logout');
    expect(captured[0].init.method).toBe('POST');
    expect(captured[0].init.credentials).toBe('include');
    const broken = (async () => {
      throw new TypeError('offline');
    }) as typeof fetch;
    await expect(signOut('http://127.0.0.1:3000', broken)).resolves.toBeUndefined();
  });

  it('shows the first letter of the email as the avatar', () => {
    expect(initialOf(person)).toBe('O');
    expect(initialOf({ email: null, personId: 'zed' })).toBe('Z');
    expect(initialOf({ email: '  ', personId: '' })).toBe('?');
  });
});
