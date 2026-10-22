/**
 * Agent-key helpers: the tiers a key can carry, the CLI and MCP snippets that
 * use a key, and the amount rules for per-currency limits. Pure functions, so
 * the screen and its tests share one definition.
 */
import type { AgentKeyLimit } from '../api/types';
import { currencyExponent, parseMajorToMinor } from './money';

export const TIERS = [
  { value: 'read', label: 'Read', hint: 'Reads the ledger. Cannot draft or post.' },
  { value: 'draft', label: 'Draft', hint: 'Reads and drafts entries. Cannot post.' },
  { value: 'post-within-limits', label: 'Post within limits', hint: 'Posts entries up to the limits below.' },
] as const;

export type TierValue = (typeof TIERS)[number]['value'];

export function tierLabel(tier: string): string {
  return TIERS.find((t) => t.value === tier)?.label ?? tier;
}

/** Placeholder shown wherever a key's token is not known (the list, the usage card). */
export const TOKEN_PLACEHOLDER = '<your-agent-key>';

/** The server origin without trailing slashes, the form the CLI and MCP config take. */
export function serverOrigin(baseUrl: string): string {
  return baseUrl.replace(/\/+$/, '');
}

/** A one-line CLI invocation that uses the key against this server. */
export function cliSnippet(baseUrl: string, token: string = TOKEN_PLACEHOLDER): string {
  return `LEDGERS_URL=${serverOrigin(baseUrl)} LEDGERS_API_KEY=${token} ledgers entries list`;
}

/** The MCP server entry for an MCP client: an HTTP server at /v1/mcp with the key as a bearer token. */
export function mcpConfig(baseUrl: string, token: string = TOKEN_PLACEHOLDER): string {
  return JSON.stringify(
    {
      mcpServers: {
        ledgers: {
          type: 'http',
          url: `${serverOrigin(baseUrl)}/v1/mcp`,
          headers: { Authorization: `Bearer ${token}` },
        },
      },
    },
    null,
    2,
  );
}

/** One currency's limit row as the form holds it. Amounts are what the person typed, in major units. */
export interface LimitRow {
  currency: string;
  perEntry: string;
  perDay: string;
}

/** Why a limit row cannot be saved, or null when it can. */
export function limitRowError(row: LimitRow): string | null {
  if (!/^[A-Z]{3}$/.test(row.currency)) return 'Use a three-letter currency code, such as EUR.';
  if (parseMajorToMinor(row.perEntry, row.currency) === null) {
    return `Enter the most one entry may move, in ${row.currency} (up to ${currencyExponent(row.currency)} decimals).`;
  }
  if (parseMajorToMinor(row.perDay, row.currency) === null) {
    return `Enter the most the key may move in a day, in ${row.currency} (up to ${currencyExponent(row.currency)} decimals).`;
  }
  return null;
}

/** Formats minor units as the canonical major-unit string the server parses ("1000.50", "500" for JPY). */
export function minorToCanonical(minor: bigint, exponent: number): string {
  const negative = minor < 0n;
  const digits = (negative ? -minor : minor).toString().padStart(exponent + 1, '0');
  const whole = exponent > 0 ? digits.slice(0, -exponent) : digits;
  const fraction = exponent > 0 ? `.${digits.slice(-exponent)}` : '';
  return `${negative ? '-' : ''}${whole}${fraction}`;
}

/**
 * The `limits` object `keys.create` takes, from the form rows. Duplicate
 * currencies keep the last row. Throws if a row is invalid; check with
 * `limitRowError` first.
 */
export function limitsPayload(rows: LimitRow[]): Record<string, AgentKeyLimit> {
  const out: Record<string, AgentKeyLimit> = {};
  for (const row of rows) {
    const exponent = currencyExponent(row.currency);
    const perEntry = parseMajorToMinor(row.perEntry, row.currency);
    const perDay = parseMajorToMinor(row.perDay, row.currency);
    if (perEntry === null || perDay === null) throw new Error(`Invalid limit for ${row.currency}`);
    out[row.currency] = {
      per_entry: minorToCanonical(perEntry, exponent),
      per_day: minorToCanonical(perDay, exponent),
    };
  }
  return out;
}
