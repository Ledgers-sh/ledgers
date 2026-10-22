/**
 * People sign-in calls (issue #12). The API holds the session in an httpOnly
 * cookie on its own origin; the app only asks whether there is a person and
 * sends them to the sign-in. No token ever reaches this code.
 */
import { ApiError, buildUrl, kindForStatus, requestInit, type ClientConfig } from './client';

/** The signed-in person as `GET /v1/auth/session` describes them. */
export interface Person {
  personId: string;
  email: string | null;
  roles: string[];
  csrfToken: string;
  expiresAt: string | null;
}

/** Where the browser goes to start a passkey sign-in on this server. */
export function signInUrl(baseUrl: string): string {
  return buildUrl(baseUrl, '/v1/auth/login');
}

/**
 * The signed-in person. Resolves to null when there is no session (401). A
 * session whose account may not use the company rejects with 403, and other
 * failures reject with the usual `ApiError`.
 */
export async function fetchSession(baseUrl: string, fetchImpl: typeof fetch = fetch): Promise<Person | null> {
  const config: ClientConfig = { baseUrl, fetch: fetchImpl };
  let response: Response;
  try {
    response = await fetchImpl(buildUrl(baseUrl, '/v1/auth/session'), requestInit(config, 'GET', { Accept: 'application/json' }));
  } catch {
    throw new ApiError('network', 'Could not reach the server. Check your connection.', null);
  }
  if (response.status === 401) return null;
  if (!response.ok) {
    const detail = await problemDetail(response);
    throw new ApiError(kindForStatus(response.status), detail ?? 'Sign-in is not available right now.', response.status, detail);
  }
  return (await response.json()) as Person;
}

/** Ends the session on the server. The cookie is cleared by the response. */
export async function signOut(baseUrl: string, fetchImpl: typeof fetch = fetch): Promise<void> {
  const config: ClientConfig = { baseUrl, fetch: fetchImpl };
  try {
    await fetchImpl(buildUrl(baseUrl, '/v1/auth/logout'), requestInit(config, 'POST', { Accept: 'application/json' }));
  } catch {
    // The page forgets the person either way; a failed call leaves the cookie to expire.
  }
}

async function problemDetail(response: Response): Promise<string | null> {
  try {
    const parsed = (await response.json()) as { detail?: unknown };
    return typeof parsed.detail === 'string' && parsed.detail.trim() !== '' ? parsed.detail : null;
  } catch {
    return null;
  }
}

/** The one or two letters an avatar shows: the email's first letter, else the person id's. */
export function initialOf(person: Pick<Person, 'email' | 'personId'>): string {
  const source = (person.email ?? person.personId ?? '').trim();
  return source ? source.charAt(0).toUpperCase() : '?';
}
