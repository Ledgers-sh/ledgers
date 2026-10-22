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

  constructor(kind: ApiErrorKind, message: string, status: number | null, detail: string | null = null) {
    super(message);
    this.name = 'ApiError';
    this.kind = kind;
    this.status = status;
    this.detail = detail;
  }
}

export interface ClientConfig {
  /** Server origin, e.g. http://127.0.0.1:3000 (no trailing slash needed). */
  baseUrl: string;
  /** An agent key. Absent for a signed-in person, whose session cookie is sent instead. */
  apiKey?: string;
  /** Injected for tests; defaults to the global fetch. */
  fetch?: typeof fetch;
}

export interface RequestSpec {
  method: 'GET' | 'POST';
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

/** The `detail` of a problem+json body, if the server sent one. */
async function readProblem(response: Response): Promise<string | null> {
  try {
    const text = await response.text();
    if (!text) return null;
    const parsed: unknown = JSON.parse(text);
    if (parsed && typeof parsed === 'object') {
      const p = parsed as { detail?: unknown };
      if (typeof p.detail === 'string' && p.detail.trim() !== '') return p.detail;
    }
    return null;
  } catch {
    return null;
  }
}

/** The init for one request: a key as bearer auth, or the person's cookie. */
export function requestInit(config: ClientConfig, method: string, headers: Record<string, string>): RequestInit {
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
    const detail = await readProblem(response);
    // Validation errors speak for themselves in the server's own words; other kinds use our wording.
    const message = kind === 'validation' && detail ? detail : MESSAGES[kind];
    throw new ApiError(kind, message, response.status, detail);
  }
  return (await response.json()) as T;
}
