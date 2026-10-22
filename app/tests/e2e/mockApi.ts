/**
 * A local fake of the Ledgers HTTP API for Playwright. It runs as a real HTTP
 * server on 127.0.0.1:3000 (route interception cannot model the CORS preflight
 * Chrome sends for a bearer-authenticated cross-origin fetch). It keeps a small
 * ledger in memory so posting and reversing show up in the journal and reports.
 */
import http from 'node:http';
import type { Page } from '@playwright/test';

/** Port of the fake API. LEDGERS_MOCK_PORT moves it when another local server holds 3000. */
const PORT = Number(process.env.LEDGERS_MOCK_PORT ?? 3000);
export const API_ORIGIN = `http://127.0.0.1:${PORT}`;
export const API_KEY = 'test_sk_mock_0123456789';

type Side = 'debit' | 'credit';
interface Line {
  account_number: string;
  side: Side;
  base: { minor: string; currency: string };
  memo: string | null;
}
interface Entry {
  external_id: string | null;
  entry_id: string;
  journal_id: string;
  fiscal_year: number;
  number: number;
  document_date: string;
  posting_date: string;
  base_currency: string;
  description: string;
  posted_by: 'agent' | 'person';
  reverses: string | null;
  reversed_by: string | null;
  recorded_at: string;
  lines: Line[];
}

export interface MockOptions {
  /** Seed entries so screens are populated (screenshots). */
  seed?: boolean;
  /**
   * Serve a person session instead of the agent key: calls with no bearer key
   * are the signed-in owner's, and the approvals fixtures are loaded.
   */
  person?: boolean;
}

export const PERSON = {
  personId: 'person_01',
  email: 'owner@example.test',
  roles: ['owner'],
  csrfToken: 'csrf_test',
  expiresAt: null,
};

type ApprovalStatus = 'pending' | 'approved' | 'rejected' | 'expired' | 'superseded';

export interface MockApproval {
  id: string;
  status: ApprovalStatus;
  command: string;
  risk: 'low' | 'high';
  amount: Record<string, string>;
  counterparty: string | null;
  reason: string;
  requested_by: string;
  created_at: string;
  expires_at: string;
  decided_at: string | null;
  decided_by: string | null;
  decision_reason: string | null;
  result: Record<string, unknown> | null;
  audit_ref: string | null;
  request_hash: string;
  preview: { lines: { account: string; side: Side; amount: string; currency: string }[]; totals: Record<string, string> } | null;
}

export interface ApprovalDecisionRecord {
  id: string;
  decision: 'approve' | 'reject';
  reason?: string;
}

const ACCOUNTS = [
  { number: '1000', name_key: 'account.cash', account_type: 'asset', normal_balance: 'debit', currency: null, parent: null, tags: [], is_postable: true },
  { number: '1200', name_key: 'account.receivables', account_type: 'asset', normal_balance: 'debit', currency: null, parent: null, tags: [], is_postable: true },
  { number: '2000', name_key: 'account.payables', account_type: 'liability', normal_balance: 'credit', currency: null, parent: null, tags: [], is_postable: true },
  { number: '3000', name_key: 'account.capital', account_type: 'equity', normal_balance: 'credit', currency: null, parent: null, tags: [], is_postable: true },
  { number: '4000', name_key: 'account.sales', account_type: 'income', normal_balance: 'credit', currency: null, parent: null, tags: [], is_postable: true },
  { number: '6000', name_key: 'account.rent', account_type: 'expense', normal_balance: 'debit', currency: null, parent: null, tags: [], is_postable: true },
  { number: '6100', name_key: 'account.software', account_type: 'expense', normal_balance: 'debit', currency: null, parent: null, tags: [], is_postable: true },
];

const period = (now = new Date()) => `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}`;

function seedEntries(): Entry[] {
  const p = period();
  const line = (account_number: string, side: Side, minor: string, memo: string | null = null): Line => ({
    account_number,
    side,
    base: { minor, currency: 'EUR' },
    memo,
  });
  return [
    {
      external_id: 'seed-1',
      entry_id: 'entry-seed-1',
      journal_id: 'general',
      fiscal_year: Number(p.slice(0, 4)),
      number: 1,
      document_date: `${p}-01`,
      posting_date: `${p}-01`,
      base_currency: 'EUR',
      description: 'Capital introduced by the owner',
      posted_by: 'person',
      reverses: null,
      reversed_by: null,
      recorded_at: `${p}-01T09:00:00Z`,
      lines: [line('1000', 'debit', '500000'), line('3000', 'credit', '500000')],
    },
    {
      external_id: 'seed-2',
      entry_id: 'entry-seed-2',
      journal_id: 'general',
      fiscal_year: Number(p.slice(0, 4)),
      number: 2,
      document_date: `${p}-03`,
      posting_date: `${p}-03`,
      base_currency: 'EUR',
      description: 'Invoice 2026-118 Halden Works GmbH',
      posted_by: 'agent',
      reverses: null,
      reversed_by: null,
      recorded_at: `${p}-03T10:15:00Z`,
      lines: [line('1200', 'debit', '119000', 'Invoice 2026-118'), line('4000', 'credit', '100000'), line('2000', 'credit', '19000')],
    },
  ];
}

function sum(entries: Entry[], filter: (account: string) => boolean, side: Side): bigint {
  let total = 0n;
  for (const entry of entries) {
    for (const line of entry.lines) {
      if (line.side === side && filter(line.account_number)) total += BigInt(line.base.minor);
    }
  }
  return total;
}

const money = (minor: bigint) => ({ minor: minor.toString(), currency: 'EUR' });

function trialRows(entries: Entry[]) {
  return ACCOUNTS.map((account) => {
    const debit = sum(entries, (n) => n === account.number, 'debit');
    const credit = sum(entries, (n) => n === account.number, 'credit');
    return { account_code: account.number, debit: money(debit), credit: money(credit), balance: money(debit - credit) };
  }).filter((row) => BigInt(row.debit.minor) !== 0n || BigInt(row.credit.minor) !== 0n);
}

export interface MockApi {
  entries: Entry[];
  posts: unknown[];
  reversals: unknown[];
  person: boolean;
  approvals: MockApproval[];
  decisions: ApprovalDecisionRecord[];
  bulk: string[][];
}

/** The real server allows the app origin via CORS; the fake does the same. */
const APP_ORIGIN = `http://localhost:${Number(process.env.LEDGERS_E2E_PORT ?? 5173)}`;
const CORS: Record<string, string> = {
  'Access-Control-Allow-Origin': APP_ORIGIN,
  'Access-Control-Allow-Headers': 'authorization, content-type',
  'Access-Control-Allow-Methods': 'GET, POST, OPTIONS',
  // The app sends the session cookie with credentials: 'include'.
  'Access-Control-Allow-Credentials': 'true',
};

interface Reply {
  status: number;
  body: unknown;
  problem?: boolean;
}

function problem(status: number, title: string, detail?: string): Reply {
  return { status, problem: true, body: { type: 'about:blank', title, status, detail } };
}

/** The operations this fake serves, in the shape the server's OpenAPI document uses. */
const OPENAPI = {
  openapi: '3.1.0',
  paths: {
    '/v1/accounts': { get: { operationId: 'accounts_list' } },
    '/v1/entries': { get: { operationId: 'entries_list' }, post: { operationId: 'entries_post' } },
    '/v1/bills/{id}/match': { post: { operationId: 'bills_match' } },
  },
};

const state: MockApi = { entries: [], posts: [], reversals: [], person: false, approvals: [], decisions: [], bulk: [] };
let nextNumber = 1;

const HOUR = 3_600_000;

/** Fixtures for the approvals inbox: pending rows of each kind, an expired one, and two decided ones. */
function approvalSeed(now = Date.now()): MockApproval[] {
  const at = (offsetMs: number) => new Date(now + offsetMs).toISOString();
  const preview = (account: string, amount: string, currency: string): MockApproval['preview'] => ({
    lines: [
      { account, side: 'debit', amount, currency },
      { account: '2000', side: 'credit', amount, currency },
    ],
    totals: { [currency]: amount },
  });
  const base = (overrides: Partial<MockApproval> & Pick<MockApproval, 'id'>): MockApproval => ({
    status: 'pending',
    command: 'entries.post',
    risk: 'low',
    amount: { EUR: '12500' },
    counterparty: null,
    reason: 'exceeds per-entry limit',
    requested_by: 'key_agent_ledger_01',
    created_at: at(-2 * HOUR),
    expires_at: at(20 * HOUR),
    decided_at: null,
    decided_by: null,
    decision_reason: null,
    result: null,
    audit_ref: null,
    request_hash: `sha256:${overrides.id.replace(/[^a-z0-9]/gi, '').padEnd(24, '0').slice(0, 24)}`,
    preview: null,
    ...overrides,
  });
  return [
    base({
      id: 'apr-low-1',
      counterparty: 'Halden Works GmbH',
      amount: { EUR: '12500' },
      reason: 'exceeds per-entry limit',
      preview: preview('6000', '12500', 'EUR'),
    }),
    base({
      id: 'apr-low-2',
      counterparty: 'Nordvik Office AS',
      amount: { EUR: '8000' },
      reason: 'supplier not on the approved list',
      preview: preview('6100', '8000', 'EUR'),
    }),
    base({
      id: 'apr-low-3',
      counterparty: 'Bayview Supplies',
      amount: { USD: '4000' },
      reason: 'new supplier',
      preview: preview('6100', '4000', 'USD'),
    }),
    base({
      id: 'apr-high-1',
      counterparty: 'Kestrel Logistics',
      risk: 'high',
      amount: { EUR: '500000' },
      reason: "exceeds the agent's daily limit",
      preview: preview('6000', '500000', 'EUR'),
    }),
    base({
      id: 'apr-expired-1',
      counterparty: 'Old Quay Ltd',
      amount: { EUR: '1000' },
      created_at: at(-30 * HOUR),
      expires_at: at(-1 * HOUR),
      preview: preview('6000', '1000', 'EUR'),
    }),
    base({
      id: 'apr-hist-1',
      status: 'approved',
      counterparty: 'Lindqvist Print',
      amount: { EUR: '45000' },
      created_at: at(-26 * HOUR),
      decided_at: at(-25 * HOUR),
      decided_by: 'person_01',
      audit_ref: 'audit_01HZKQ7M',
      result: { entry: { entry_id: 'entry-apr-hist-1', journal_id: 'general', fiscal_year: 2026, number: 1 } },
    }),
    base({
      id: 'apr-hist-2',
      status: 'rejected',
      counterparty: 'Pell & Ross',
      amount: { EUR: '900' },
      created_at: at(-50 * HOUR),
      decided_at: at(-49 * HOUR),
      decided_by: 'person_01',
      decision_reason: 'Duplicate of invoice 2026-101',
      audit_ref: 'audit_01HZKQ2P',
    }),
  ];
}

function approvalView(item: MockApproval, withPreview: boolean) {
  const { preview, ...rest } = item;
  return withPreview ? { ...rest, preview } : rest;
}

function problemBody(status: number, slug: string, title: string, detail: string): Reply {
  return {
    status,
    problem: true,
    body: { type: `https://ledgers.sh/problems/${slug}`, title, status, detail },
  };
}

function findApproval(id: string): MockApproval | undefined {
  return state.approvals.find((a) => a.id === id);
}

/** A decision as the server makes it: expiry is checked first, then the decision is written. */
function decideOne(item: MockApproval, decision: 'approve' | 'reject', reason: string | undefined): Reply | null {
  if (item.status !== 'pending') return problemBody(409, 'approval-not-pending', 'Conflict', 'This request is no longer pending.');
  if (Date.parse(item.expires_at) <= Date.now()) {
    item.status = 'expired';
    return problemBody(409, 'approval-expired', 'Conflict', 'This request expired before it was decided.');
  }
  if (decision === 'approve' && item.risk === 'high') {
    return problemBody(401, 'reauth-required', 'Sign in again', 'Approving a high-risk request needs a fresh sign-in.');
  }
  item.decided_at = new Date().toISOString();
  item.decided_by = PERSON.personId;
  item.audit_ref = `audit_mock_${item.id}`;
  if (decision === 'approve') {
    item.status = 'approved';
    item.result = { entry: { entry_id: `entry-${item.id}`, journal_id: 'general', fiscal_year: 2026, number: 100 + state.approvals.indexOf(item) } };
  } else {
    item.status = 'rejected';
    item.decision_reason = reason ?? null;
  }
  state.decisions.push({ id: item.id, decision, ...(reason !== undefined ? { reason } : {}) });
  return null;
}

/** The approvals routes, for the signed-in person. Returns null when the path is not an approvals route. */
function handleApprovals(method: string, url: URL, bodyText: string): Reply | null {
  const path = url.pathname;
  if (method === 'GET' && path === '/v1/auth/session') return { status: 200, body: PERSON };
  if (method === 'GET' && path === '/v1/approval-requests') {
    const status = url.searchParams.get('status') ?? 'pending';
    const limit = Number(url.searchParams.get('limit') ?? '50');
    const pendingCount = state.approvals.filter((a) => a.status === 'pending').length;
    const wanted = status === 'decided' ? (a: MockApproval) => a.status !== 'pending' : (a: MockApproval) => a.status === 'pending';
    const items = state.approvals
      .filter(wanted)
      .sort((a, b) => b.created_at.localeCompare(a.created_at))
      .slice(0, limit)
      .map((a) => approvalView(a, false));
    return { status: 200, body: { pending_count: pendingCount, items } };
  }
  const one = /^\/v1\/approval-requests\/([^/]+)$/.exec(path);
  if (method === 'GET' && one) {
    const item = findApproval(decodeURIComponent(one[1]));
    return item ? { status: 200, body: approvalView(item, true) } : problemBody(404, 'not-found', 'Not found', 'No such approval request.');
  }
  const decision = /^\/v1\/approval-requests\/([^/]+)\/decision$/.exec(path);
  if (method === 'POST' && decision) {
    const item = findApproval(decodeURIComponent(decision[1]));
    if (!item) return problemBody(404, 'not-found', 'Not found', 'No such approval request.');
    const body = JSON.parse(bodyText || '{}') as { decision: 'approve' | 'reject'; reason?: string };
    const failed = decideOne(item, body.decision, body.reason);
    if (failed) return failed;
    return { status: 200, body: { approval: approvalView(item, false), result: item.result } };
  }
  if (method === 'POST' && path === '/v1/approval-requests/bulk-decision') {
    const body = JSON.parse(bodyText) as { ids: string[]; decision: 'approve' };
    const items = body.ids.map(findApproval);
    if (items.some((item) => item === undefined)) return problemBody(404, 'not-found', 'Not found', 'An id is unknown.');
    const picked = items as MockApproval[];
    const currencies = new Set(picked.flatMap((item) => Object.keys(item.amount)));
    if (picked.some((item) => item.risk === 'high') || currencies.size > 1) {
      return problemBody(422, 'bulk-selection-refused', 'Selection refused', 'Select low-risk requests in one currency only. Nothing was approved.');
    }
    state.bulk.push(body.ids);
    const results = picked.map((item) => {
      decideOne(item, 'approve', undefined);
      return { id: item.id, approval: approvalView(item, false), result: item.result };
    });
    return { status: 200, body: { results } };
  }
  return null;
}

/** Routes one request against the in-memory ledger. */
/** An agent key as the fake keeps it. The token is held here only, like the server's hash. */
interface MockKey {
  id: string;
  name: string;
  prefix: string;
  tier: string;
  limits: Record<string, { per_entry: string; per_day: string }>;
  created_at: string;
  created_by: string;
  last_used_at: string | null;
  revoked_at: string | null;
  token: string;
}

let keyCounter = 0;
const keyState: { keys: MockKey[] } = { keys: [] };

function keyView(key: MockKey) {
  const { token: _token, ...view } = key;
  return view;
}

function hex(length: number, seed: number): string {
  return Array.from({ length }, (_, i) => ((seed * 31 + i * 7) % 16).toString(16)).join('');
}

/** Key commands, for the signed-in person (keys.list, keys.create, keys.revoke). */
function handleKeys(method: string, path: string, bodyText: string): Reply | null {
  if (method === 'GET' && path === '/v1/keys') return { status: 200, body: { keys: keyState.keys.map(keyView) } };
  if (method === 'POST' && path === '/v1/keys') {
    const input = JSON.parse(bodyText || '{}') as { name?: string; tier?: string; limits?: MockKey['limits'] };
    const name = (input.name ?? '').trim();
    if (name === '') return problem(400, 'Invalid input', 'name is required');
    keyCounter += 1;
    const prefix = `lk_live_${hex(16, keyCounter)}`;
    const token = `${prefix}_${hex(64, keyCounter + 3)}`;
    const key: MockKey = {
      id: `01KEY${String(keyCounter).padStart(20, '0')}`,
      name,
      prefix,
      tier: input.tier ?? 'read',
      limits: input.limits ?? {},
      created_at: '2026-10-22T10:00:00Z',
      created_by: PERSON.personId,
      last_used_at: null,
      revoked_at: null,
      token,
    };
    keyState.keys.push(key);
    return { status: 200, body: { key: keyView(key), token } };
  }
  const revoke = /^\/v1\/keys\/([^/]+)\/revoke$/.exec(path);
  if (method === 'POST' && revoke) {
    const key = keyState.keys.find((k) => k.id === decodeURIComponent(revoke[1]));
    if (!key) return problem(404, 'Not found');
    key.revoked_at ??= '2026-10-22T11:00:00Z';
    return { status: 200, body: { key: keyView(key) } };
  }
  return null;
}

function handle(method: string, url: URL, bodyText: string): Reply {
  const path = url.pathname;
  const approvals = handleApprovals(method, url, bodyText);
  if (approvals) return approvals;
  if (path.startsWith('/v1/keys')) {
    const keys = handleKeys(method, path, bodyText);
    if (keys) return keys;
  }
  if (method === 'GET' && path === '/v1/auth/session' && state.person) return { status: 200, body: PERSON };

  if (method === 'GET' && path === '/v1/accounts') return { status: 200, body: { accounts: ACCOUNTS } };
  if (method === 'GET' && path === '/v1/entries') return { status: 200, body: { entries: state.entries } };
  if (method === 'GET' && path.startsWith('/v1/entries/')) {
    const id = decodeURIComponent(path.slice('/v1/entries/'.length));
    const entry = state.entries.find((e) => e.external_id === id);
    return entry ? { status: 200, body: entry } : problem(404, 'Not found');
  }
  if (method === 'GET' && path === '/v1/reports/trial-balance') {
    return { status: 200, body: { period: url.searchParams.get('period'), rows: trialRows(state.entries) } };
  }
  if (method === 'GET' && path === '/v1/reports/p-and-l') {
    const revenue = sum(state.entries, (n) => n.startsWith('4'), 'credit') - sum(state.entries, (n) => n.startsWith('4'), 'debit');
    const expenses = sum(state.entries, (n) => n.startsWith('6'), 'debit') - sum(state.entries, (n) => n.startsWith('6'), 'credit');
    return {
      status: 200,
      body: { period: url.searchParams.get('period'), revenue: money(revenue), expenses: money(expenses), net_income: money(revenue - expenses) },
    };
  }
  if (method === 'GET' && path === '/v1/reports/balance-sheet') {
    const assets = sum(state.entries, (n) => n.startsWith('1'), 'debit') - sum(state.entries, (n) => n.startsWith('1'), 'credit');
    const liabilities = sum(state.entries, (n) => n.startsWith('2'), 'credit') - sum(state.entries, (n) => n.startsWith('2'), 'debit');
    const equity = sum(state.entries, (n) => n.startsWith('3'), 'credit') - sum(state.entries, (n) => n.startsWith('3'), 'debit');
    return {
      status: 200,
      body: {
        as_of: url.searchParams.get('as_of'),
        assets: money(assets),
        liabilities: money(liabilities),
        // Equity includes the period's net income, as the real balance sheet does.
        equity: money(equity + netIncome(state.entries)),
      },
    };
  }
  if (method === 'POST' && path === '/v1/entries') {
    const body = JSON.parse(bodyText) as {
      journal_id: string;
      fiscal_year: number;
      entry: { external_id: string | null; description: string; posting_date: string; base_currency: string; lines: Line[] };
    };
    const debit = body.entry.lines.filter((l) => l.side === 'debit').reduce((a, l) => a + BigInt(l.base.minor), 0n);
    const credit = body.entry.lines.filter((l) => l.side === 'credit').reduce((a, l) => a + BigInt(l.base.minor), 0n);
    if (debit !== credit) return problem(422, 'Validation failed', 'entry does not balance');
    state.posts.push(body);
    const existing = state.entries.find((e) => e.external_id === body.entry.external_id);
    if (existing) return { status: 200, body: { entry: summary(existing) } };
    const number = nextNumber++;
    const entry: Entry = {
      external_id: body.entry.external_id,
      entry_id: `entry-${number}`,
      journal_id: body.journal_id,
      fiscal_year: body.fiscal_year,
      number,
      document_date: body.entry.posting_date,
      posting_date: body.entry.posting_date,
      base_currency: body.entry.base_currency,
      description: body.entry.description,
      posted_by: 'person',
      reverses: null,
      reversed_by: null,
      recorded_at: `${body.entry.posting_date}T12:00:00Z`,
      lines: body.entry.lines,
    };
    state.entries.push(entry);
    return { status: 200, body: { entry: summary(entry) } };
  }
  const reverse = /^\/v1\/entries\/([^/]+)\/reverse$/.exec(path);
  if (method === 'POST' && reverse) {
    const original = state.entries.find((e) => e.external_id === decodeURIComponent(reverse[1]));
    if (!original) return problem(404, 'Not found');
    if (original.reversed_by) return problem(422, 'Validation failed', 'entry already reversed');
    const body = JSON.parse(bodyText) as { posting_date: string; description: string };
    state.reversals.push(body);
    const number = nextNumber++;
    const reversal: Entry = {
      ...original,
      external_id: null,
      entry_id: `entry-${number}`,
      number,
      posting_date: body.posting_date,
      document_date: body.posting_date,
      description: body.description,
      reverses: original.entry_id,
      reversed_by: null,
      lines: original.lines.map((l) => ({ ...l, side: l.side === 'debit' ? 'credit' : 'debit' })),
    };
    original.reversed_by = reversal.entry_id;
    state.entries.push(reversal);
    return { status: 200, body: { entry: summary(reversal) } };
  }
  if (method === 'POST' && /^\/v1\/bills\/[^/]+\/match$/.test(path)) {
    return { status: 200, body: { status: 'no_candidates', candidates: [] } };
  }
  return problem(501, 'Not yet implemented');
}

function netIncome(entries: Entry[]): bigint {
  const revenue = sum(entries, (n) => n.startsWith('4'), 'credit') - sum(entries, (n) => n.startsWith('4'), 'debit');
  const expenses = sum(entries, (n) => n.startsWith('6'), 'debit') - sum(entries, (n) => n.startsWith('6'), 'credit');
  return revenue - expenses;
}

function summary(entry: Entry) {
  return {
    entry_id: entry.entry_id,
    journal_id: entry.journal_id,
    fiscal_year: entry.fiscal_year,
    number: entry.number,
    recorded_at: entry.recorded_at,
  };
}

let server: http.Server | null = null;

/**
 * Starts (once per worker) the fake API on 127.0.0.1:3000 and resets its ledger.
 * Requests without the right bearer key get 401.
 */
export async function mockApi(options: MockOptions = {}): Promise<MockApi> {
  state.entries = options.seed ? seedEntries() : [];
  state.posts = [];
  state.reversals = [];
  state.person = options.person ?? false;
  state.approvals = options.person ? approvalSeed() : [];
  state.decisions = [];
  state.bulk = [];
  keyState.keys = [];
  keyCounter = 0;
  nextNumber = state.entries.length + 1;
  if (!server) {
    server = http.createServer((req, res) => {
      let text = '';
      req.on('data', (chunk: Buffer) => (text += chunk.toString()));
      req.on('end', () => {
        const headers = { ...CORS };
        if (req.method === 'OPTIONS') {
          res.writeHead(204, headers);
          res.end();
          return;
        }
        if (req.method === 'GET' && new URL(req.url ?? '/', API_ORIGIN).pathname === '/v1/openapi') {
          // The public document the app reads to decide which pages to show.
          res.writeHead(200, { ...headers, 'Content-Type': 'application/json' });
          res.end(JSON.stringify(OPENAPI));
          return;
        }
        // A person session: calls without a bearer key are the signed-in owner's.
        const asPerson = state.person && !req.headers.authorization;
        const path = new URL(req.url ?? '/', API_ORIGIN).pathname;
        // Signed out: no key and no session. The people sign-in answers 401 here,
        // which the app shows as the sign-in screen. Specs mock signed-in sessions.
        if (!asPerson && path === '/v1/auth/session' && req.headers.authorization !== `Bearer ${API_KEY}`) {
          res.writeHead(401, { ...headers, 'Content-Type': 'application/problem+json' });
          res.end(JSON.stringify(problem(401, 'Sign-in required').body));
          return;
        }
        if (!asPerson && req.headers.authorization !== `Bearer ${API_KEY}`) {
          res.writeHead(401, { ...headers, 'Content-Type': 'application/problem+json' });
          res.end(JSON.stringify(problem(401, 'Unauthorized').body));
          return;
        }
        // Agent keys may not manage keys or review approvals (the server answers 403).
        if (path.startsWith('/v1/keys') && req.headers.authorization === `Bearer ${API_KEY}`) {
          res.writeHead(403, { ...headers, 'Content-Type': 'application/problem+json' });
          res.end(JSON.stringify(problem(403, 'Human-only', 'this command is human-only').body));
          return;
        }
        if (!asPerson && path.startsWith('/v1/approval-requests')) {
          res.writeHead(403, { ...headers, 'Content-Type': 'application/problem+json' });
          res.end(JSON.stringify(problem(403, 'Forbidden').body));
          return;
        }
        const reply = handle(req.method ?? 'GET', new URL(req.url ?? '/', API_ORIGIN), text);
        res.writeHead(reply.status, { ...headers, 'Content-Type': reply.problem ? 'application/problem+json' : 'application/json' });
        res.end(JSON.stringify(reply.body));
      });
    });
    await new Promise<void>((resolve, reject) => {
      server?.once('error', reject);
      server?.listen(PORT, '127.0.0.1', () => resolve());
    });
  }
  return state;
}

/** Stops the fake API (call from afterAll). */
export async function stopMockApi(): Promise<void> {
  const current = server;
  server = null;
  if (current) await new Promise<void>((resolve) => current.close(() => resolve()));
}

/** Puts the API key in sessionStorage before any page script runs (skips the connect screen). */
export async function signIn(page: Page, theme: 'dark' | 'light' = 'dark') {
  await page.addInitScript(
    ({ key, origin, themeValue }) => {
      window.sessionStorage.setItem('ledgers.apiKey', key);
      window.sessionStorage.setItem('ledgers.baseUrl', origin);
      window.localStorage.setItem('ledgers.theme', themeValue);
    },
    { key: API_KEY, origin: API_ORIGIN, themeValue: theme },
  );
}
