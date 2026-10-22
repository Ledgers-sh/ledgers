import { describe, expect, it } from 'vitest';
import { NotAMemberError, fetchSession, signInUrl } from './auth';
import { invitationAccept, invitationRevoke, memberLeave, memberRemove, memberSetRole, peopleList } from './people';
import { ApiError } from './client';

interface Captured {
  url: string;
  method: string;
  body: unknown;
}

/** A fetch stand-in that answers one canned response and records the request. */
function fakeFetch(status: number, body: unknown = {}) {
  const captured: Captured[] = [];
  const fetchImpl = (async (input: RequestInfo | URL, init?: RequestInit) => {
    captured.push({
      url: String(input),
      method: init?.method ?? 'GET',
      body: init?.body === undefined ? undefined : JSON.parse(String(init.body)),
    });
    if (status === 204) return new Response(null, { status: 204 });
    return new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } });
  }) as typeof fetch;
  return { fetchImpl, captured };
}

const BASE = 'http://127.0.0.1:3000';

describe('people calls', () => {
  it('reads the roster with the session cookie and no bearer key', async () => {
    const { fetchImpl, captured } = fakeFetch(200, { members: [], me: { role: 'viewer' }, canManage: false });
    await peopleList({ baseUrl: BASE, fetch: fetchImpl });
    expect(captured[0].url).toBe(`${BASE}/v1/auth/people`);
    expect(captured[0].method).toBe('GET');
  });

  it('changes a role with PATCH on the member, and sends only the role', async () => {
    const { fetchImpl, captured } = fakeFetch(200, { personId: 'p_1', role: 'admin' });
    await memberSetRole({ baseUrl: BASE, fetch: fetchImpl }, 'co_1', 'p_1', 'admin');
    expect(captured[0]).toEqual({ url: `${BASE}/v1/orgs/co_1/members/p_1`, method: 'PATCH', body: { role: 'admin' } });
  });

  it('removes a member with DELETE and reads the empty 204 without failing', async () => {
    const { fetchImpl, captured } = fakeFetch(204);
    await expect(memberRemove({ baseUrl: BASE, fetch: fetchImpl }, 'co_1', 'p_2')).resolves.toBeUndefined();
    expect(captured[0].method).toBe('DELETE');
    expect(captured[0].url).toBe(`${BASE}/v1/orgs/co_1/members/p_2`);
  });

  it('revokes an invitation by id and leaves the company with POST', async () => {
    const revoke = fakeFetch(204);
    await invitationRevoke({ baseUrl: BASE, fetch: revoke.fetchImpl }, 'inv_9');
    expect(revoke.captured[0]).toEqual({ url: `${BASE}/v1/auth/invitations/inv_9`, method: 'DELETE', body: undefined });

    const leave = fakeFetch(204);
    await memberLeave({ baseUrl: BASE, fetch: leave.fetchImpl }, 'co_1');
    expect(leave.captured[0]).toEqual({ url: `${BASE}/v1/orgs/co_1/leave`, method: 'POST', body: undefined });
  });

  it('accepts an invitation by posting its token', async () => {
    const { fetchImpl, captured } = fakeFetch(200, {});
    await invitationAccept({ baseUrl: BASE, fetch: fetchImpl }, 'tok_1');
    expect(captured[0]).toEqual({ url: `${BASE}/v1/orgs/invitations/accept`, method: 'POST', body: { token: 'tok_1' } });
  });

  it('surfaces a refusal with its status and slug', async () => {
    const { fetchImpl } = fakeFetch(409, { type: 'https://ledgers.sh/problems/orgs-last-owner', title: 'Last owner', detail: 'The company must keep an owner.' });
    const error = await memberLeave({ baseUrl: BASE, fetch: fetchImpl }, 'co_1').catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ApiError);
    expect((error as ApiError).status).toBe(409);
    expect((error as ApiError).slug).toBe('orgs-last-owner');
  });
});

describe('session for a signed-in account that is not a member', () => {
  it('throws NotAMemberError with the server title and detail for a not-a-member 403', async () => {
    const { fetchImpl } = fakeFetch(403, {
      type: 'https://ledgers.sh/problems/not-a-member',
      title: "You're not a member of this company yet",
      detail: 'Ask the owner for an invitation.',
    });
    const error = await fetchSession(BASE, fetchImpl).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(NotAMemberError);
    expect((error as NotAMemberError).title).toBe("You're not a member of this company yet");
    expect((error as NotAMemberError).detail).toBe('Ask the owner for an invitation.');
  });

  it('keeps a plain 403 as an ApiError', async () => {
    const { fetchImpl } = fakeFetch(403, { detail: 'only the owner can use this ledger.' });
    const error = await fetchSession(BASE, fetchImpl).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ApiError);
    expect(error).not.toBeInstanceOf(NotAMemberError);
  });

  it('sends the sign-in with a return path when asked', () => {
    expect(signInUrl(BASE, '/accept?token=tok_1')).toBe(`${BASE}/v1/auth/login?return_to=%2Faccept%3Ftoken%3Dtok_1`);
    expect(signInUrl(BASE)).toBe(`${BASE}/v1/auth/login`);
  });
});
