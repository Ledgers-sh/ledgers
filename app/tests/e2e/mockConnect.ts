/**
 * The connected-sources routes of the fake API (issue #142): connections, their
 * runs, the allocation rules and the costs by unit. Demo data exists only when
 * a test asks for it (`connect: true`); otherwise every list is empty, the way
 * a company with no connections answers. A token sent on create or update is
 * accepted and never echoed back, which the tests check.
 */

export interface ConnectReply {
  status: number;
  body: unknown;
  problem?: boolean;
}

interface MockConnection {
  id: string;
  connector: string;
  connector_name: string;
  name: string;
  config: Record<string, unknown>;
  default_unit: string;
  is_demo: boolean;
  has_credential: boolean;
  last_sync_at: string | null;
  created_at: string;
  updated_at: string;
}

interface MockRun {
  id: string;
  window_since: string;
  window_until: string;
  status: string;
  invoices_found: number;
  bills_created: number;
  usage_rows: number;
  error_message: string | null;
  notes: string[];
  finished_at: string;
}

/** Every write the fake API accepted, and every token it was sent, for assertions. */
export const connectState: {
  connections: MockConnection[];
  runs: Record<string, MockRun[]>;
  rules: Record<string, { pattern: string; unit_code: string }[]>;
  unmapped: Record<string, { resource_kind: string; resource_id: string; resource_name: string; periods: string[]; suggested_unit: string | null }[]>;
  lines: { period: string; unit_code: string; connector: string; connection_id: string; currency: string; amount_minor: number }[];
  writes: { method: string; path: string; body: unknown }[];
  tokensReceived: string[];
  /** The credential objects sent for multi-field connectors (never echoed back). */
  credentialsReceived: Record<string, string>[];
} = {
  connections: [],
  runs: {},
  rules: {},
  unmapped: {},
  lines: [],
  writes: [],
  tokensReceived: [],
  credentialsReceived: [],
};

const CONNECTORS = [
  {
    id: 'cloudflare',
    name: 'Cloudflare',
    credential_fields: [{ name: 'token', label: 'API token (Billing Read, Account Analytics Read)' }],
    hosts: ['api.cloudflare.com'],
    config_keys: ['account_id'],
  },
  {
    id: 'anthropic',
    name: 'Anthropic',
    credential_fields: [{ name: 'token', label: 'Admin API key (organization admin)' }],
    hosts: ['api.anthropic.com'],
    config_keys: [],
  },
  {
    id: 'planetscale',
    name: 'PlanetScale',
    credential_fields: [{ name: 'token', label: 'Service token as ID:token (read_invoices)' }],
    hosts: ['api.planetscale.com'],
    config_keys: ['organization', 'currency'],
  },
  {
    id: 'aws',
    name: 'Amazon Web Services',
    credential_fields: [
      { name: 'access_key_id', label: 'Access key ID (IAM user with ce:GetCostAndUsage only)' },
      { name: 'secret_access_key', label: 'Secret access key' },
    ],
    hosts: ['ce.us-east-1.amazonaws.com'],
    config_keys: ['tag_key'],
  },
  {
    id: 'vercel',
    name: 'Vercel',
    credential_fields: [{ name: 'token', label: 'Access token (team Billing role)' }],
    hosts: ['api.vercel.com'],
    config_keys: ['team_id'],
  },
  {
    id: 'gcp',
    name: 'Google Cloud',
    credential_fields: [{ name: 'service_account_json', label: 'Service account key (JSON)' }],
    hosts: ['bigquery.googleapis.com', 'oauth2.googleapis.com'],
    config_keys: ['project', 'dataset', 'table'],
  },
  {
    id: 'azure',
    name: 'Microsoft Azure',
    credential_fields: [{ name: 'client_secret', label: 'Client secret' }],
    hosts: ['login.microsoftonline.com', 'management.azure.com'],
    config_keys: ['tenant_id', 'client_id', 'subscription_id'],
  },
  {
    id: 'supabase',
    name: 'Supabase',
    credential_fields: [{ name: 'token', label: 'Personal access token' }],
    hosts: ['api.supabase.com'],
    config_keys: ['organization'],
  },
  {
    id: 'openai',
    name: 'OpenAI',
    credential_fields: [{ name: 'token', label: 'Admin API key (organization owner or admin)' }],
    hosts: ['api.openai.com'],
    config_keys: [],
  },
  {
    id: 'spaceship',
    name: 'Spaceship',
    credential_fields: [
      { name: 'api_key', label: 'API key' },
      { name: 'api_secret', label: 'API secret' },
    ],
    hosts: ['spaceship.dev'],
    config_keys: [],
  },
  {
    id: 'twilio',
    name: 'Twilio',
    credential_fields: [
      { name: 'api_key_sid', label: 'API key SID (starts with SK)' },
      { name: 'api_key_secret', label: 'API key secret' },
    ],
    hosts: ['api.twilio.com'],
    config_keys: ['account_sid', 'subaccount_sid'],
  },
  {
    id: 'meta_ads',
    name: 'Meta Ads',
    credential_fields: [{ name: 'access_token', label: 'System-user access token (ads_read only)' }],
    hosts: ['graph.facebook.com'],
    config_keys: ['ad_account_id', 'name_prefixes'],
  },
  {
    id: 'google_ads',
    name: 'Google Ads',
    credential_fields: [
      { name: 'developer_token', label: 'Developer token' },
      { name: 'client_id', label: 'OAuth client ID' },
      { name: 'client_secret', label: 'OAuth client secret' },
      { name: 'refresh_token', label: 'Refresh token (scope adwords)' },
    ],
    hosts: ['googleads.googleapis.com', 'oauth2.googleapis.com'],
    config_keys: ['customer_id', 'login_customer_id', 'name_prefixes'],
  },
  {
    id: 'jira',
    name: 'Jira (Atlassian)',
    credential_fields: [
      { name: 'email', label: 'Account email' },
      { name: 'api_token', label: 'API token' },
    ],
    hosts: [],
    config_keys: ['site', 'price_per_seat', 'price_currency'],
  },
  {
    id: 'polar',
    name: 'Polar',
    credential_fields: [{ name: 'token', label: 'Organization access token (orders:read, refunds:read)' }],
    hosts: ['api.polar.sh'],
    config_keys: ['clearing_account', 'revenue_account', 'tax_account'],
  },
  {
    id: 'stripe',
    name: 'Stripe',
    credential_fields: [{ name: 'token', label: 'Restricted API key (read access)' }],
    hosts: ['api.stripe.com'],
    config_keys: ['clearing_account', 'revenue_account', 'suspense_account', 'stripe_account'],
  },
  {
    id: 'openrouter',
    name: 'OpenRouter',
    credential_fields: [{ name: 'token', label: 'Management API key' }],
    hosts: ['openrouter.ai'],
    config_keys: [],
  },
];

const MONTHS = ['2026-01', '2026-02', '2026-03', '2026-04', '2026-05', '2026-06', '2026-07', '2026-08', '2026-09', '2026-10'];

/** Demo state: three demo connections, their runs, rules, a needs-mapping queue and the monthly costs. */
export function seedConnect(): void {
  const now = '2026-10-22T08:00:00Z';
  connectState.connections = [
    { id: 'conn-cf-demo', connector: 'cloudflare', connector_name: 'Cloudflare', name: 'Cloudflare (demo)', config: { account_id: 'demo0000000000000000000000000001', expense_account: '6100', vat_rate_bp: 0 }, default_unit: 'FZ', is_demo: true, has_credential: false, last_sync_at: now, created_at: '2026-01-05T09:00:00Z', updated_at: now },
    { id: 'conn-ant-demo', connector: 'anthropic', connector_name: 'Anthropic', name: 'Anthropic (demo)', config: { expense_account: '6200', vat_rate_bp: 0 }, default_unit: 'FZ', is_demo: true, has_credential: false, last_sync_at: now, created_at: '2026-01-05T09:01:00Z', updated_at: now },
    { id: 'conn-ps-demo', connector: 'planetscale', connector_name: 'PlanetScale', name: 'PlanetScale (demo)', config: { organization: 'factory-zero-demo', currency: 'EUR', expense_account: '6300', vat_rate_bp: 0 }, default_unit: 'FZ', is_demo: true, has_credential: false, last_sync_at: now, created_at: '2026-01-05T09:02:00Z', updated_at: now },
  ];
  connectState.runs = {
    'conn-cf-demo': [
      { id: 'run-cf-2', window_since: '2026-01-01', window_until: '2026-12-31', status: 'succeeded', invoices_found: 10, bills_created: 0, usage_rows: 110, error_message: null, notes: ['R2 operations and storage are not read: the dataset is not confirmed for this account; R2 spend books to the default unit.', '1 resource(s) have no allocation rule and booked to the default unit: worker:tmp-preview-worker'], finished_at: now },
      { id: 'run-cf-1', window_since: '2026-01-01', window_until: '2026-12-31', status: 'failed', invoices_found: 0, bills_created: 0, usage_rows: 0, error_message: 'api.cloudflare.com refused the token (403); check the token scopes', notes: [], finished_at: '2026-10-01T08:00:00Z' },
    ],
    'conn-ant-demo': [],
    'conn-ps-demo': [],
  };
  connectState.rules = {
    'conn-cf-demo': [
      { pattern: 'fz-ledgers-db', unit_code: 'LDG' },
      { pattern: 'ledgers-*', unit_code: 'LDG' },
      { pattern: 'sqeezin-*', unit_code: 'SQZ' },
      { pattern: 'fz-*', unit_code: 'FZ' },
    ],
    'conn-ant-demo': [
      { pattern: 'apikey_ledgers_web', unit_code: 'LDG' },
      { pattern: 'apikey_sqeezin', unit_code: 'SQZ' },
    ],
    'conn-ps-demo': [],
  };
  connectState.unmapped = {
    'conn-cf-demo': [
      { resource_kind: 'worker', resource_id: 'tmp-preview-worker', resource_name: 'tmp-preview-worker', periods: ['2026-09', '2026-10'], suggested_unit: null },
      { resource_kind: 'worker', resource_id: 'sqeezin-worker', resource_name: 'sqeezin-worker', periods: ['2026-10'], suggested_unit: 'SQZ' },
    ],
    'conn-ant-demo': [{ resource_kind: 'api-key', resource_id: 'apikey_cratefield', resource_name: 'apikey_cratefield', periods: ['2026-10'], suggested_unit: null }],
    'conn-ps-demo': [{ resource_kind: 'organization', resource_id: 'factory-zero-demo', resource_name: 'factory-zero-demo', periods: ['2026-01', '2026-10'], suggested_unit: 'FZ' }],
  };
  // Costs: Cloudflare and PlanetScale in EUR, the mock company's currency; Anthropic in USD (not charted).
  const shares: Record<string, number> = { FZ: 0.3, LDG: 0.45, SQZ: 0.25 };
  connectState.lines = [];
  MONTHS.forEach((period, index) => {
    for (const [unit, share] of Object.entries(shares)) {
      connectState.lines.push({ period, unit_code: unit, connector: 'cloudflare', connection_id: 'conn-cf-demo', currency: 'EUR', amount_minor: Math.round((9_600 + index * 450) * share) });
      connectState.lines.push({ period, unit_code: unit, connector: 'planetscale', connection_id: 'conn-ps-demo', currency: 'EUR', amount_minor: Math.round((4_900 + Math.floor(index / 3) * 650) * share) });
    }
    connectState.lines.push({ period, unit_code: 'LDG', connector: 'anthropic', connection_id: 'conn-ant-demo', currency: 'USD', amount_minor: 180_000 + index * 9_000 });
    connectState.lines.push({ period, unit_code: 'SQZ', connector: 'anthropic', connection_id: 'conn-ant-demo', currency: 'USD', amount_minor: 90_000 + index * 4_000 });
  });
}

export function resetConnect(seed: boolean): void {
  connectState.connections = [];
  connectState.runs = {};
  connectState.rules = {};
  connectState.unmapped = {};
  connectState.lines = [];
  connectState.writes = [];
  connectState.tokensReceived = [];
  connectState.credentialsReceived = [];
  if (seed) seedConnect();
}

function reply(status: number, body: unknown): ConnectReply {
  return { status, body };
}

function problem(status: number, title: string, detail: string): ConnectReply {
  return { status, body: { type: 'about:blank', title, status, detail }, problem: true };
}

function newId(): string {
  return `conn-${connectState.connections.length + 1}-${Math.random().toString(36).slice(2, 7)}`;
}

/** Routes the connections paths; null for paths that are not connections. */
export function handleConnect(method: string, url: URL, bodyText: string, agent: boolean): ConnectReply | null {
  const path = url.pathname;
  if (!path.startsWith('/v1/connections') && !path.startsWith('/v1/allocation-rules') && path !== '/v1/reports/costs-by-unit') return null;
  let body: Record<string, unknown> = {};
  try {
    body = bodyText ? (JSON.parse(bodyText) as Record<string, unknown>) : {};
  } catch {
    body = {};
  }
  const writes = method !== 'GET';
  if (writes) connectState.writes.push({ method, path, body });
  if (writes && agent) return problem(403, 'human-only', 'connections are changed by a person');

  if (method === 'GET' && path === '/v1/connections') {
    return reply(200, { connections: connectState.connections.map(({ has_credential, ...rest }) => ({ ...rest, has_credential })), connectors: CONNECTORS });
  }
  if (method === 'POST' && path === '/v1/connections') {
    const token = typeof body.token === 'string' ? body.token : '';
    if (token) connectState.tokensReceived.push(token);
    if (body.credential && typeof body.credential === 'object') {
      connectState.credentialsReceived.push(body.credential as Record<string, string>);
    }
    const id = newId();
    const now = '2026-10-23T09:00:00Z';
    const connector = CONNECTORS.find((c) => c.id === body.connector);
    if (!connector) return problem(422, 'invalid-input', 'Invalid input');
    const view: MockConnection = {
      id,
      connector: connector.id,
      connector_name: connector.name,
      name: String(body.name ?? connector.name),
      config: (body.config as Record<string, unknown>) ?? {},
      default_unit: String(body.default_unit ?? 'FZ'),
      is_demo: false,
      has_credential: true,
      last_sync_at: null,
      created_at: now,
      updated_at: now,
    };
    connectState.connections.push(view);
    connectState.runs[id] = [];
    connectState.rules[id] = [];
    connectState.unmapped[id] = [];
    return reply(200, view);
  }
  const match = /^\/v1\/connections\/([^/]+)\/(update|delete|sync|runs|test|renewals)$/.exec(path);
  if (match) {
    const [, id, action] = match;
    const connection = connectState.connections.find((c) => c.id === id);
    if (!connection) return problem(422, 'invalid-input', `connection ${id} does not exist`);
    if (method === 'GET' && action === 'runs') return reply(200, { runs: connectState.runs[id] ?? [] });
    if (method === 'POST' && action === 'renewals') {
      // A connection named "Broken" stands for a registrar that refuses the key.
      if (connection.name.startsWith('Broken')) {
        return reply(200, { ok: false, error: 'spaceship.dev refused the key (401)', as_of: '2026-10-25', window_days: 60, domains: [], notes: [] });
      }
      // The registrar's domains: one expires inside the 60-day window, one has lapsed.
      const domains = [
        { name: 'squeez.in', expires_on: '2026-11-20', auto_renew: true, status: 'active' },
        { name: 'lapsed.example', expires_on: '2026-10-01', auto_renew: false, status: 'redemption' },
      ];
      return reply(200, { ok: true, error: null, as_of: '2026-10-25', window_days: 60, domains, notes: [] });
    }
    if (method === 'POST' && action === 'test') {
      // A connection named "Broken" stands for a vendor that refuses the credential.
      if (connection.name.startsWith('Broken')) return reply(200, { ok: false, notes: [], error: 'api.vercel.com refused the token (403); check the token scopes' });
      return reply(200, { ok: true, notes: ['The vendor answered for today.'], error: null });
    }
    if (method === 'POST' && action === 'delete') {
      connectState.connections = connectState.connections.filter((c) => c.id !== id);
      return reply(200, { id, disconnected: true });
    }
    if (method === 'POST' && action === 'sync') {
      const run: MockRun = {
        id: `run-${Date.now()}`,
        window_since: '2026-01-01',
        window_until: '2026-12-31',
        status: 'succeeded',
        invoices_found: 2,
        bills_created: 0,
        usage_rows: 4,
        error_message: null,
        notes: [],
        finished_at: '2026-10-23T09:05:00Z',
      };
      connectState.runs[id] = [run, ...(connectState.runs[id] ?? [])];
      connection.last_sync_at = run.finished_at;
      return reply(200, run);
    }
    if (method === 'POST' && action === 'update') {
      if (typeof body.token === 'string') connectState.tokensReceived.push(body.token);
      if (typeof body.name === 'string') connection.name = body.name;
      return reply(200, { ...connection });
    }
  }
  if (method === 'GET' && path === '/v1/allocation-rules') {
    const only = url.searchParams.get('connection_id');
    const rows = connectState.connections
      .filter((c) => !only || c.id === only)
      .map((c) => ({
        connection_id: c.id,
        name: c.name,
        default_unit: c.default_unit,
        rules: connectState.rules[c.id] ?? [],
        unmapped: connectState.unmapped[c.id] ?? [],
      }));
    return reply(200, {
      connections: rows,
      units: [
        { code: 'FZ', name: 'Factory Zero holding' },
        { code: 'LDG', name: 'Ledgers' },
        { code: 'SQZ', name: 'Sqeezin' },
      ],
    });
  }
  const rulesMatch = /^\/v1\/allocation-rules\/([^/]+)$/.exec(path);
  if (rulesMatch && method === 'POST') {
    const id = rulesMatch[1];
    const rules = (body.rules as { pattern: string; unit_code: string }[]) ?? [];
    connectState.rules[id] = rules;
    const queue = connectState.unmapped[id] ?? [];
    const remaining = queue.filter((resource) => !rules.some((rule) => rule.pattern === resource.resource_id));
    connectState.unmapped[id] = remaining;
    return reply(200, { rules, resplit: 1, kept_frozen: 0 });
  }
  if (method === 'GET' && path === '/v1/reports/costs-by-unit') {
    const from = url.searchParams.get('from') ?? '';
    const to = url.searchParams.get('to') ?? '';
    return reply(200, { lines: connectState.lines.filter((line) => line.period >= from && line.period <= to) });
  }
  return problem(404, 'not-found', 'no such route');
}
