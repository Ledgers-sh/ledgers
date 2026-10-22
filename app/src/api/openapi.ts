import { buildUrl } from './client';

/**
 * The server's OpenAPI document (`GET /v1/openapi`), read at runtime. The
 * document is public; no key is sent. Any failure reads as "not available".
 */
export async function fetchOpenApiDocument(baseUrl: string, fetchImpl: typeof fetch = fetch): Promise<unknown | null> {
  try {
    const response = await fetchImpl(buildUrl(baseUrl, '/v1/openapi'), { headers: { Accept: 'application/json' } });
    if (!response.ok) return null;
    return await response.json();
  } catch {
    return null;
  }
}
