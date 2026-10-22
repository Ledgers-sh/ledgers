/**
 * People sign-in calls (issue #12). The API holds the session in an httpOnly
 * cookie on its own origin; the app only asks whether there is a person and
 * sends them to the sign-in. No token ever reaches this code.
 */
import { ApiError, buildUrl, kindForStatus, problemSlug, requestInit, type ClientConfig } from './client';

/** The signed-in person as `GET /v1/auth/session` describes them. */
export interface Person {
  personId: string;
  email: string | null;
  /** The company this session is for. */
  companyId: string;
  /** The person's role in that company (owner, admin, accountant, approver, viewer or auditor). */
  role: string;
  roles: string[];
  csrfToken: string;
  expiresAt: string | null;
}

/**
 * A signed-in account that holds no membership in the company (403 `not-a-member`).
 * It is still signed in: the People screens can accept an invitation from here.
 */
export class NotAMemberError extends Error {
  readonly title: string;
  readonly detail: string | null;

  constructor(title: string, detail: string | null) {
    super(title);
    this.name = 'NotAMemberError';
    this.title = title;
    this.detail = detail;
  }
}

/**
 * Where the browser goes to start a passkey sign-in on this server. With
 * `returnTo` (an app path such as `/accept?token=...`) the server sends the
 * browser back there once the person has signed in.
 */
export function signInUrl(baseUrl: string, returnTo?: string): string {
  return buildUrl(baseUrl, '/v1/auth/login', { return_to: returnTo });
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
    const problem = await problemFields(response);
    if (response.status === 403 && problem.slug === 'not-a-member') {
      throw new NotAMemberError(problem.title ?? "You're not a member of this company yet", problem.detail);
    }
    const detail = problem.detail;
    throw new ApiError(kindForStatus(response.status), detail ?? 'Sign-in is not available right now.', response.status, detail, problem.slug);
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

/** The title, detail and slug of a problem+json body; nulls for anything the body does not say. */
async function problemFields(response: Response): Promise<{ title: string | null; detail: string | null; slug: string | null }> {
  try {
    const parsed = (await response.json()) as Record<string, unknown>;
    const text = (value: unknown) => (typeof value === 'string' && value.trim() !== '' ? value : null);
    return { title: text(parsed.title), detail: text(parsed.detail), slug: problemSlug(parsed) };
  } catch {
    return { title: null, detail: null, slug: null };
  }
}

/** The one or two letters an avatar shows: the email's first letter, else the person id's. */
export function initialOf(person: Pick<Person, 'email' | 'personId'>): string {
  const source = (person.email ?? person.personId ?? '').trim();
  return source ? source.charAt(0).toUpperCase() : '?';
}
