import { describe, expect, it } from 'vitest';
import { cliSnippet, limitRowError, limitsPayload, mcpConfig, minorToCanonical, serverOrigin, TOKEN_PLACEHOLDER, tierLabelKey } from './agentKeys';

describe('usage snippets', () => {
  const token = 'lk_live_0123456789abcdef_' + 'ab'.repeat(32);

  it('builds the CLI line from the server URL and the key', () => {
    expect(cliSnippet('https://api.ledgers.sh/', token)).toBe(
      `LEDGERS_URL=https://api.ledgers.sh LEDGERS_API_KEY=${token} ledgers entries list`,
    );
  });

  it('uses a placeholder when no token is known', () => {
    expect(cliSnippet('http://127.0.0.1:3000')).toContain(`LEDGERS_API_KEY=${TOKEN_PLACEHOLDER}`);
    expect(mcpConfig('http://127.0.0.1:3000')).toContain(`Bearer ${TOKEN_PLACEHOLDER}`);
  });

  it('builds an MCP HTTP server entry at /v1/mcp with the key as bearer auth', () => {
    const parsed = JSON.parse(mcpConfig('https://api.ledgers.sh//', token));
    expect(parsed).toEqual({
      mcpServers: {
        ledgers: {
          type: 'http',
          url: 'https://api.ledgers.sh/v1/mcp',
          headers: { Authorization: `Bearer ${token}` },
        },
      },
    });
  });

  it('strips trailing slashes from the origin', () => {
    expect(serverOrigin('https://x.test///')).toBe('https://x.test');
  });
});

describe('tiers', () => {
  it('names the three tiers with message keys, and passes unknown values through', () => {
    expect(tierLabelKey('read')).toBe('agentKeys.tier_read');
    expect(tierLabelKey('draft')).toBe('agentKeys.tier_draft');
    expect(tierLabelKey('post-within-limits')).toBe('agentKeys.tier_post');
    expect(tierLabelKey('admin')).toBeNull();
  });
});

describe('limit rows', () => {
  it('accepts major amounts in the currency exponent and rejects the rest', () => {
    expect(limitRowError({ currency: 'EUR', perEntry: '1000.50', perDay: '5000' })).toBeNull();
    expect(limitRowError({ currency: 'EUR', perEntry: '1,000.50', perDay: '0' })).toBeNull();
    expect(limitRowError({ currency: 'EUR', perEntry: '', perDay: '1' })).not.toBeNull();
    expect(limitRowError({ currency: 'EUR', perEntry: '1.005', perDay: '1' })).not.toBeNull();
    expect(limitRowError({ currency: 'EUR', perEntry: '-1', perDay: '1' })).not.toBeNull();
    expect(limitRowError({ currency: 'eur', perEntry: '1', perDay: '1' })).not.toBeNull();
    expect(limitRowError({ currency: 'JPY', perEntry: '1.5', perDay: '1' })).not.toBeNull();
  });

  it('sends canonical strings the server parses, in each exponent', () => {
    expect(
      limitsPayload([
        { currency: 'EUR', perEntry: '1,000.5', perDay: '5000' },
        { currency: 'JPY', perEntry: '500', perDay: '5000' },
        { currency: 'BHD', perEntry: '1.2', perDay: '10' },
      ]),
    ).toEqual({
      EUR: { per_entry: '1000.50', per_day: '5000.00' },
      JPY: { per_entry: '500', per_day: '5000' },
      BHD: { per_entry: '1.200', per_day: '10.000' },
    });
  });

  it('formats minor units canonically', () => {
    expect(minorToCanonical(0n, 2)).toBe('0.00');
    expect(minorToCanonical(5n, 2)).toBe('0.05');
    expect(minorToCanonical(100050n, 2)).toBe('1000.50');
    expect(minorToCanonical(7n, 0)).toBe('7');
  });

  it('throws rather than sending an invalid row', () => {
    expect(() => limitsPayload([{ currency: 'EUR', perEntry: 'x', perDay: '1' }])).toThrow();
  });
});
