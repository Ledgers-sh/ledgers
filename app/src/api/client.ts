/**
 * Low-level HTTP client. A client with an agent key sends
 * `Authorization: Bearer <key>`; a signed-in person's client sends the session
 * cookie (`credentials: 'include'`) and no key. Parses problem+json errors and
 * maps HTTP status to an `ApiError` kind.
 */

export type ApiErrorKind =
  | 'unauthorized' // 401: key missing, wrong or revoked; or no person session
  | 'forbidden' // 403: policy denied the command for this key
  | 'validation' // 400/422: the server rejected the input
  | 'not_found' // 404
  | 'unavailable' // 501/503: not wired or down
  | 'server' // other 5xx
  | 'network'; // fetch failed (offline, DNS, CORS)

export class ApiError extends Error {
  readonly kind: ApiErrorKind;
  readonly status: number | null;
  readonly detail: string | null;
  /** The problem slug the server named, e.g. "reauth-required" or "approval-not-pending"; null when absent. */
  readonly slug: string | null;

  constructor(
    kind: ApiErrorKind,
    message: string,
    status: number | null,
    detail: string | null = null,
    slug: string | null = null,
  ) {
    super(message);
    this.name = 'ApiError';
    this.kind = kind;
    this.status = status;
    this.detail = detail;
    this.slug = slug;
  }
}

/** The header that names the company a request acts for (the server's `X-Ledgers-Company`). */
export const COMPANY_HEADER = 'X-Ledgers-Company';

/**
 * The company every request from this page acts for: the one in the URL
 * (`?company=`), or null for the deployment's own company. The shell sets it
 * before any request is made and whenever the company changes.
 */
let activeCompany: string | null = null;

export function setActiveCompany(company: string | null): void {
  activeCompany = company;
}

export function getActiveCompany(): string | null {
  return activeCompany;
}

export interface ClientConfig {
  /** Server origin, e.g. http://127.0.0.1:3000 (no trailing slash needed). */
  baseUrl: string;
  /** An agent key. Absent for a signed-in person, whose session cookie is sent instead. */
  apiKey?: string;
  /** Injected for tests; defaults to the global fetch. */
  fetch?: typeof fetch;
  /**
   * The company this one call acts for, when it is not the page's company (a group
   * screen reads each side's chart). Absent: the page's company.
   */
  company?: string;
}

export interface RequestSpec {
  method: 'GET' | 'POST' | 'PATCH' | 'DELETE';
  path: string;
  query?: Record<string, string | undefined>;
  body?: unknown;
}

export function buildUrl(baseUrl: string, path: string, query?: Record<string, string | undefined>): string {
  const base = baseUrl.replace(/\/+$/, '');
  const url = new URL(base + path);
  if (query) {
    for (const [key, value] of Object.entries(query)) {
      if (value !== undefined && value !== '') url.searchParams.set(key, value);
    }
  }
  return url.toString();
}

/** Maps an HTTP status to the error kind the UI shows. */
export function kindForStatus(status: number): ApiErrorKind {
  if (status === 401) return 'unauthorized';
  if (status === 403) return 'forbidden';
  if (status === 400 || status === 422) return 'validation';
  if (status === 404) return 'not_found';
  if (status === 501 || status === 503) return 'unavailable';
  return 'server';
}

const MESSAGES: Record<ApiErrorKind, string> = {
  unauthorized: 'Your sign-in has ended, or the API key was rejected. Sign in again or check the key.',
  forbidden: 'This key is not allowed to run that command.',
  validation: 'The server rejected the input.',
  not_found: 'Not found on the server.',
  unavailable: 'The server does not offer this yet.',
  server: 'The server failed to process the request.',
  network: 'Could not reach the server. Check the URL and your connection.',
};

export interface Problem {
  detail: string | null;
  slug: string | null;
}

/**
 * The problem slug of a problem+json body: an explicit `slug` member, else the
 * last segment of a `type` URI or URN (`.../reauth-required`, `urn:...:reauth-required`).
 */
export function problemSlug(parsed: Record<string, unknown>): string | null {
  if (typeof parsed.slug === 'string' && parsed.slug.trim() !== '') return parsed.slug.trim();
  if (typeof parsed.type === 'string' && parsed.type !== 'about:blank') {
    const last = parsed.type.split(/[/:#]/).filter(Boolean).pop();
    if (last) return last;
  }
  return null;
}

/** The `detail` and slug of a problem+json body, if the server sent them. */
async function readProblem(response: Response): Promise<Problem> {
  try {
    const text = await response.text();
    if (!text) return { detail: null, slug: null };
    const parsed: unknown = JSON.parse(text);
    if (parsed && typeof parsed === 'object') {
      const p = parsed as Record<string, unknown>;
      const detail = typeof p.detail === 'string' && p.detail.trim() !== '' ? p.detail : null;
      return { detail, slug: problemSlug(p) };
    }
    return { detail: null, slug: null };
  } catch {
    return { detail: null, slug: null };
  }
}

/** The init for one request: a key as bearer auth, or the person's cookie, and the company it acts for. */
export function requestInit(config: ClientConfig, method: string, headers: Record<string, string>): RequestInit {
  const company = config.company ?? activeCompany;
  if (company) headers[COMPANY_HEADER] = company;
  if (config.apiKey) {
    headers.Authorization = `Bearer ${config.apiKey}`;
    return { method, headers, cache: 'no-store' };
  }
  return { method, headers, cache: 'no-store', credentials: 'include' };
}

export async function request<T>(config: ClientConfig, spec: RequestSpec): Promise<T> {
  const doFetch = config.fetch ?? fetch;
  const headers: Record<string, string> = { Accept: 'application/json' };
  const init = requestInit(config, spec.method, headers);
  if (spec.body !== undefined) {
    headers['Content-Type'] = 'application/json';
    init.body = JSON.stringify(spec.body);
  }

  let response: Response;
  try {
    response = await doFetch(buildUrl(config.baseUrl, spec.path, spec.query), init);
  } catch {
    throw new ApiError('network', MESSAGES.network, null);
  }

  if (!response.ok) {
    const kind = kindForStatus(response.status);
    const problem = await readProblem(response);
    // Validation errors speak for themselves in the server's own words; other kinds use our wording.
    const message = kind === 'validation' && problem.detail ? problem.detail : MESSAGES[kind];
    throw new ApiError(kind, message, response.status, problem.detail, problem.slug);
  }
  // 204 No Content (a delete): there is no body to read.
  if (response.status === 204) return undefined as T;
  return (await response.json()) as T;
}
