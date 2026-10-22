/**
 * Build-time configuration. `VITE_LEDGERS_API` sets the server origin the
 * Connect screen starts from (for example https://api-staging.ledgers.sh when
 * the app is built for staging). The user can still edit it on that screen.
 */

/** The local development server, used when no build-time URL is set. */
export const LOCAL_SERVER_URL = 'http://127.0.0.1:3000';

/** The default server origin for a build-time value, without a trailing slash. */
export function defaultServerUrl(configured: string | undefined): string {
  const trimmed = configured?.trim() ?? '';
  return trimmed === '' ? LOCAL_SERVER_URL : trimmed.replace(/\/+$/, '');
}
