import { describe, expect, it } from 'vitest';
import { operationIds } from './features';
import { fetchOpenApiDocument } from './openapi';
import { defaultServerUrl, LOCAL_SERVER_URL } from './config';

const document = {
  openapi: '3.1.0',
  paths: {
    '/v1/accounts': { get: { operationId: 'accounts_list' } },
    '/v1/entries': { get: { operationId: 'entries_list' }, post: { operationId: 'entries_post' } },
    '/v1/bills/{id}/post': { post: { operationId: 'bills_post' } },
  },
};

describe('operationIds', () => {
  it('lists every operationId in the paths', () => {
    expect([...operationIds(document)].sort()).toEqual(
      ['accounts_list', 'bills_post', 'entries_list', 'entries_post'].sort(),
    );
  });

  it('lists nothing for a document without paths or a non-object', () => {
    expect(operationIds({}).size).toBe(0);
    expect(operationIds(null).size).toBe(0);
    expect(operationIds('openapi').size).toBe(0);
    expect(operationIds({ paths: 3 }).size).toBe(0);
  });

  it('ignores entries that are not operations', () => {
    expect(operationIds({ paths: { '/x': null, '/y': { get: 'nope' }, '/z': { post: { operationId: 7 } } } }).size).toBe(0);
  });
});

describe('reading the server’s operations', () => {
  it('reads the public document without sending a key', async () => {
    const seen: { url: string; init?: RequestInit }[] = [];
    const fetchImpl = (async (url: string, init?: RequestInit) => {
      seen.push({ url, init });
      return new Response(JSON.stringify(document), { status: 200 });
    }) as typeof fetch;
    const ids = operationIds(await fetchOpenApiDocument('https://api-staging.ledgers.sh/', fetchImpl));
    expect(ids.has('bills_post')).toBe(true);
    expect(seen[0].url).toBe('https://api-staging.ledgers.sh/v1/openapi');
    expect(new Headers(seen[0].init?.headers).has('Authorization')).toBe(false);
  });

  it('reads as nothing offered when the server fails or is unreachable', async () => {
    const down = (async () => new Response('', { status: 503 })) as typeof fetch;
    expect(operationIds(await fetchOpenApiDocument('http://127.0.0.1:3000', down)).size).toBe(0);
    const broken = (async () => {
      throw new TypeError('network');
    }) as typeof fetch;
    expect(operationIds(await fetchOpenApiDocument('http://127.0.0.1:3000', broken)).size).toBe(0);
  });
});

describe('defaultServerUrl', () => {
  it('falls back to the local server when no build-time URL is set', () => {
    expect(defaultServerUrl(undefined)).toBe(LOCAL_SERVER_URL);
    expect(defaultServerUrl('  ')).toBe(LOCAL_SERVER_URL);
  });

  it('uses the build-time URL without a trailing slash', () => {
    expect(defaultServerUrl('https://api-staging.ledgers.sh/')).toBe('https://api-staging.ledgers.sh');
  });
});
