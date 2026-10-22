/**
 * Which operations the server offers, read from its OpenAPI document
 * (`GET /v1/openapi`, generated from the command registry). The app shows a
 * page only when the server lists the operation behind it, so no control is
 * ever a dead button. Any failure reads as "nothing offered".
 */

/** The operationIds an OpenAPI 3 document lists. A malformed document lists none. */
export function operationIds(document: unknown): Set<string> {
  const ids = new Set<string>();
  const paths = document && typeof document === 'object' ? (document as { paths?: unknown }).paths : undefined;
  if (!paths || typeof paths !== 'object') return ids;
  for (const item of Object.values(paths)) {
    if (!item || typeof item !== 'object') continue;
    for (const operation of Object.values(item)) {
      const id = operation && typeof operation === 'object' ? (operation as { operationId?: unknown }).operationId : undefined;
      if (typeof id === 'string') ids.add(id);
    }
  }
  return ids;
}
