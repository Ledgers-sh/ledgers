/**
 * Agent-key helpers: the tiers a key can carry, the CLI and MCP snippets that
 * use a key, and the amount rules for per-currency limits. Pure functions, so
 * the screen and its tests share one definition.
 */
import type { AgentKeyLimit } from '../api/types';
import type { MessageKey } from '../i18n';
import { currencyExponent, parseMajorToMinor } from './money';

/** The tiers a key can carry. Labels and hints are message keys, so the screen words them in the reader's language. */
export const TIERS = [
  { value: 'read', labelKey: 'agentKeys.tier_read', hintKey: 'agentKeys.tier_read_hint' },
  { value: 'draft', labelKey: 'agentKeys.tier_draft', hintKey: 'agentKeys.tier_draft_hint' },
  { value: 'post-within-limits', labelKey: 'agentKeys.tier_post', hintKey: 'agentKeys.tier_post_hint' },
] as const satisfies readonly { value: string; labelKey: MessageKey; hintKey: MessageKey }[];

export type TierValue = (typeof TIERS)[number]['value'];

/** The message key for a tier's label, or null for a tier the app does not know (the server's own word is shown then). */
export function tierLabelKey(tier: string): MessageKey | null {
  return TIERS.find((t) => t.value === tier)?.labelKey ?? null;
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

/** Why a limit row cannot be saved. The screen words it in the reader's language. */
export type LimitProblem =
  | { kind: 'currency' }
  | { kind: 'per-entry'; currency: string; decimals: number }
  | { kind: 'per-day'; currency: string; decimals: number };

/** Why a limit row cannot be saved, or null when it can. */
export function limitRowError(row: LimitRow): LimitProblem | null {
  if (!/^[A-Z]{3}$/.test(row.currency)) return { kind: 'currency' };
  const decimals = currencyExponent(row.currency);
  if (parseMajorToMinor(row.perEntry, row.currency) === null) return { kind: 'per-entry', currency: row.currency, decimals };
  if (parseMajorToMinor(row.perDay, row.currency) === null) return { kind: 'per-day', currency: row.currency, decimals };
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
