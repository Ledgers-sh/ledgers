import { describe, expect, it } from 'vitest';
import { ApiError } from './client';
import { approvalDecide, approvalGet, approvalsBulkApprove, approvalsList } from './approvals';

interface Captured {
  url: string;
  init: RequestInit;
}

function fakeFetch(status: number, body: unknown = {}) {
  const captured: Captured[] = [];
  const fetchImpl = (async (input: RequestInfo | URL, init?: RequestInit) => {
    captured.push({ url: String(input), init: init ?? {} });
    return new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } });
  }) as typeof fetch;
  return { fetchImpl, captured };
}

const config = (fetchImpl: typeof fetch) => ({ baseUrl: 'http://127.0.0.1:3000/', fetch: fetchImpl });

describe('approvals client', () => {
  it('lists pending requests with the count and a limit, using the session cookie (no bearer key)', async () => {
    const { fetchImpl, captured } = fakeFetch(200, { pending_count: 2, items: [] });
    const list = await approvalsList(config(fetchImpl), 'pending', 1);
    expect(list.pending_count).toBe(2);
    expect(captured[0].url).toBe('http://127.0.0.1:3000/v1/approval-requests?status=pending&limit=1');
    expect(captured[0].init.credentials).toBe('include');
    expect(new Headers(captured[0].init.headers as HeadersInit).has('Authorization')).toBe(false);
  });

  it('asks for decided requests with the decided status', async () => {
    const { fetchImpl, captured } = fakeFetch(200, { pending_count: 0, items: [] });
    await approvalsList(config(fetchImpl), 'decided');
    expect(captured[0].url).toBe('http://127.0.0.1:3000/v1/approval-requests?status=decided&limit=50');
  });

  it('gets one request by id, encoding the id in the path', async () => {
    const { fetchImpl, captured } = fakeFetch(200, { id: 'apr/1' });
    await approvalGet(config(fetchImpl), 'apr/1');
    expect(captured[0].url).toBe('http://127.0.0.1:3000/v1/approval-requests/apr%2F1');
    expect(captured[0].init.method).toBe('GET');
  });

  it('posts an approve decision with no reason field', async () => {
    const { fetchImpl, captured } = fakeFetch(200, { approval: { id: 'a1' }, result: null });
    await approvalDecide(config(fetchImpl), 'a1', { decision: 'approve' });
    expect(captured[0].url).toBe('http://127.0.0.1:3000/v1/approval-requests/a1/decision');
    expect(captured[0].init.method).toBe('POST');
    expect(JSON.parse(String(captured[0].init.body))).toEqual({ decision: 'approve' });
  });

  it('posts a reject decision with its reason', async () => {
    const { fetchImpl, captured } = fakeFetch(200, { approval: { id: 'a1' } });
    await approvalDecide(config(fetchImpl), 'a1', { decision: 'reject', reason: 'Duplicate invoice' });
    expect(JSON.parse(String(captured[0].init.body))).toEqual({ decision: 'reject', reason: 'Duplicate invoice' });
  });

  it('posts the bulk approval with its ids', async () => {
    const { fetchImpl, captured } = fakeFetch(200, { results: [{ id: 'a1', approval: { id: 'a1' } }] });
    const output = await approvalsBulkApprove(config(fetchImpl), ['a1', 'a2']);
    expect(captured[0].url).toBe('http://127.0.0.1:3000/v1/approval-requests/bulk-decision');
    expect(JSON.parse(String(captured[0].init.body))).toEqual({ ids: ['a1', 'a2'], decision: 'approve' });
    expect(output.results[0].id).toBe('a1');
  });

  it('keeps the problem slug of a 401 reauth-required so the page can send the person to sign in', async () => {
    const problem = { type: 'https://ledgers.sh/problems/reauth-required', title: 'Sign in again', status: 401, detail: 'Sign in again to approve.' };
    const captured: string[] = [];
    const fetchImpl = (async (input: RequestInfo | URL) => {
      captured.push(String(input));
      return new Response(JSON.stringify(problem), { status: 401, headers: { 'Content-Type': 'application/problem+json' } });
    }) as typeof fetch;
    const error = await approvalDecide(config(fetchImpl), 'a1', { decision: 'approve' }).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ApiError);
    expect((error as ApiError).slug).toBe('reauth-required');
    expect((error as ApiError).status).toBe(401);
  });

  it('reads an explicit slug member and a urn type, and reports none for about:blank', async () => {
    const run = async (body: unknown) => {
      const { fetchImpl } = fakeFetch(409, body);
      return approvalDecide(config(fetchImpl), 'a1', { decision: 'approve' }).catch((e: unknown) => (e as ApiError).slug);
    };
    expect(await run({ slug: 'approval-expired', detail: 'expired' })).toBe('approval-expired');
    expect(await run({ type: 'urn:ledgers:problem:approval-not-pending', detail: 'x' })).toBe('approval-not-pending');
    expect(await run({ type: 'about:blank', title: 'Conflict' })).toBeNull();
  });
});
