/**
 * A local fake of the Ledgers HTTP API for Playwright. It runs as a real HTTP
 * server on 127.0.0.1:3000 (route interception cannot model the CORS preflight
 * Chrome sends for a bearer-authenticated cross-origin fetch). It keeps a small
 * ledger in memory so posting and reversing show up in the journal and reports.
 */
import { readFileSync } from 'node:fs';
import { handleBills, resetBills, servedFile } from './mockBills';
import { handleSettings, resetSettings } from './mockSettings';
import http from 'node:http';
import { fileURLToPath } from 'node:url';
import type { Page } from '@playwright/test';

/** Port of the fake API. LEDGERS_MOCK_PORT moves it when another local server holds 3000. */
const PORT = Number(process.env.LEDGERS_MOCK_PORT ?? 3000);
/** Port of the app under test. LEDGERS_E2E_PORT moves it when another local server holds 5173. */
const WEB_PORT = Number(process.env.LEDGERS_E2E_PORT ?? 5173);
export const API_ORIGIN = `http://127.0.0.1:${PORT}`;
export const WEB_ORIGIN = `http://localhost:${WEB_PORT}`;
export const API_KEY = 'test_sk_mock_0123456789';

type Side = 'debit' | 'credit';
interface Line {
  account_number: string;
  side: Side;
  base: { minor: string; currency: string };
  memo: string | null;
  /** Dimension tags by kind, as the journal stores them. */
  dimensions?: Record<string, string>;
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
  /**
   * Adds this many generated entries after the seed, for paging: the first
   * has a hyphenated external id (like the demo book's), the second has none.
   */
  extraEntries?: number;
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
  { number: '1000', name_key: 'account.generic_sme.cash', account_type: 'asset', normal_balance: 'debit', currency: null, parent: null, tags: [], is_postable: true },
  { number: '1200', name_key: 'account.generic_sme.accounts_receivable', account_type: 'asset', normal_balance: 'debit', currency: null, parent: null, tags: [], is_postable: true },
  { number: '2000', name_key: 'account.generic_sme.accounts_payable', account_type: 'liability', normal_balance: 'credit', currency: null, parent: null, tags: [], is_postable: true },
  { number: '3000', name_key: 'account.generic_sme.share_capital', account_type: 'equity', normal_balance: 'credit', currency: null, parent: null, tags: [], is_postable: true },
  { number: '4000', name_key: 'account.generic_sme.sales', account_type: 'income', normal_balance: 'credit', currency: null, parent: null, tags: [], is_postable: true },
  { number: '6000', name_key: 'account.generic_sme.rent', account_type: 'expense', normal_balance: 'debit', currency: null, parent: null, tags: [], is_postable: true },
  { number: '6100', name_key: 'account.generic_sme.office_supplies', account_type: 'expense', normal_balance: 'debit', currency: null, parent: null, tags: [], is_postable: true },
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
      lines: [line('1200', 'debit', '119000', 'Invoice 2026-118'),
        { ...line('4000', 'credit', '100000'), dimensions: { project: 'PRJ-1' } },
        line('2000', 'credit', '19000')],
    },
    {
      external_id: 'seed-3',
      entry_id: 'entry-seed-3',
      journal_id: 'general',
      fiscal_year: Number(p.slice(0, 4)),
      number: 3,
      document_date: `${p}-05`,
      posting_date: `${p}-05`,
      base_currency: 'EUR',
      description: 'Rent for the office',
      posted_by: 'person',
      reverses: null,
      reversed_by: null,
      recorded_at: `${p}-05T09:30:00Z`,
      lines: [
        { ...line('6000', 'debit', '250000'), dimensions: { 'cost_centre': 'CC-ADMIN' } },
        line('1000', 'credit', '250000'),
      ],
    },
    {
      external_id: 'seed-4',
      entry_id: 'entry-seed-4',
      journal_id: 'general',
      fiscal_year: Number(p.slice(0, 4)),
      number: 4,
      document_date: `${p}-08`,
      posting_date: `${p}-08`,
      base_currency: 'EUR',
      description: 'Supplier invoice: office supplies',
      posted_by: 'agent',
      reverses: null,
      reversed_by: null,
      recorded_at: `${p}-08T11:00:00Z`,
      lines: [
        { ...line('6100', 'debit', '45000'), dimensions: { 'cost_centre': 'CC-SALES' } },
        line('2000', 'credit', '45000'),
      ],
    },
  ];
}

/** Generated entries for the journal paging tests (see MockOptions.extraEntries). */
function extraEntriesFor(count: number): Entry[] {
  const p = period();
  return Array.from({ length: count }, (_, index) => {
    const number = index + 3;
    const day = String((index % 27) + 1).padStart(2, '0');
    const externalId = index === 0 ? 'halden-12-bank-charges-and-interest' : index === 1 ? null : `bulk-${number}`;
    return {
      external_id: externalId,
      entry_id: index === 0 ? 'demo-01M5HTGY741A4WFVVT1WSVJYP4' : index === 1 ? 'demo-01M5HTFRTE2D7Y0M9YXBDZECM4' : `entry-bulk-${number}`,
      journal_id: 'general',
      fiscal_year: Number(p.slice(0, 4)),
      number,
      document_date: `${p}-${day}`,
      posting_date: `${p}-${day}`,
      base_currency: 'EUR',
      description: index === 0 ? 'Bank charges and loan interest, Dezember' : `Generated entry ${number}`,
      posted_by: index % 2 ? 'person' : 'agent',
      reverses: null,
      reversed_by: null,
      recorded_at: `${p}-${day}T08:00:00Z`,
      lines: [
        { account_number: '5100', side: 'debit', base: { minor: '990', currency: 'EUR' }, memo: null },
        { account_number: '1200', side: 'credit', base: { minor: '990', currency: 'EUR' }, memo: null },
      ],
    };
  });
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

/** One account of the fake chart, as the server keeps it (the view adds the derived fields). */
interface MockAccount {
  number: string;
  name_key: string;
  display_name: string | null;
  account_type: string;
  normal_balance: string;
  currency: string | null;
  parent: string | null;
  tags: string[];
  archived: boolean;
}

export interface MockApi {
  accounts: MockAccount[];
  entries: Entry[];
  posts: unknown[];
  reversals: unknown[];
  person: boolean;
  approvals: MockApproval[];
  decisions: ApprovalDecisionRecord[];
  bulk: string[][];
}

/** The real server allows the app origin via CORS; the fake does the same. */
const CORS: Record<string, string> = {
  'Access-Control-Allow-Origin': WEB_ORIGIN,
  'Access-Control-Allow-Headers': 'authorization, content-type, x-file-name',
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

/**
 * The operations this fake serves: the document the real server generates for
 * its registry (app/tests/fixtures/openapi.json, checked by the Rust test
 * openapi_surfaces), so the palette lists what the server lists.
 */
const OPENAPI = JSON.parse(readFileSync(fileURLToPath(new URL('../fixtures/openapi.json', import.meta.url)), 'utf8')) as Record<string, unknown>;

const state: MockApi = { accounts: [], entries: [], posts: [], reversals: [], person: false, approvals: [], decisions: [], bulk: [] };
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

  if (path.startsWith('/v1/accounts')) {
    const accounts = handleAccounts(method, path, bodyText);
    if (accounts) return accounts;
  }
  if (method === 'GET' && path === '/v1/entries') {
    // Newest first, one page at a time, as the server pages it: `after` is the last entry_id of the previous page.
    const limit = Math.min(Math.max(Number(url.searchParams.get('limit') ?? 50), 1), 200);
    const newestFirst = [...state.entries].reverse();
    const after = url.searchParams.get('after');
    let start = 0;
    if (after) {
      const index = newestFirst.findIndex((entry) => entry.entry_id === after);
      if (index < 0) return problem(404, 'Not found');
      start = index + 1;
    }
    return { status: 200, body: { entries: newestFirst.slice(start, start + limit) } };
  }
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
  if (method === 'GET' && path === '/v1/reports/trends') {
    return trendsReply(url);
  }
  if (method === 'GET' && path === '/v1/reports/balance-sheet') {
    const assets = sum(state.entries, (n) => n.startsWith('1'), 'debit') - sum(state.entries, (n) => n.startsWith('1'), 'credit');
    const liabilities = sum(state.entries, (n) => n.startsWith('2'), 'credit') - sum(state.entries, (n) => n.startsWith('2'), 'debit');
    const equity = sum(state.entries, (n) => n.startsWith('3'), 'credit') - sum(state.entries, (n) => n.startsWith('3'), 'debit');
    const result = netIncome(state.entries);
    const difference = assets - liabilities - (equity + result);
    return {
      status: 200,
      body: {
        as_of: url.searchParams.get('as_of'),
        assets: money(assets),
        liabilities: money(liabilities),
        // Equity includes the current year result, as the real balance sheet does.
        equity: money(equity + result),
        equity_accounts: money(equity),
        current_year_result: money(result),
        difference: money(difference),
        balanced: difference === 0n,
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
    for (const line of body.entry.lines) {
      if (findAccount(line.account_number)?.archived) {
        return problemBody(422, 'journal-entry-refused', 'Validation failed', `account ${line.account_number} is archived and takes no new postings`);
      }
    }
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

/** The chart the fake starts with: the seed accounts, a heading with a child, and one archived account. */
function seedAccounts(): MockAccount[] {
  const base: MockAccount[] = ACCOUNTS.map((account) => ({
    number: account.number,
    name_key: account.name_key,
    display_name: null,
    account_type: account.account_type,
    normal_balance: account.normal_balance,
    currency: null,
    parent: null,
    tags: [],
    archived: false,
  }));
  return [
    ...base,
    { number: '1500', name_key: 'account.fake.fixed_assets', display_name: 'Fixed assets', account_type: 'asset', normal_balance: 'debit', currency: null, parent: null, tags: [], archived: false },
    { number: '1510', name_key: 'account.fake.office_equipment', display_name: 'Office equipment', account_type: 'asset', normal_balance: 'debit', currency: null, parent: '1500', tags: ['equipment'], archived: false },
    { number: '6900', name_key: 'account.fake.old_telephone', display_name: 'Old telephone', account_type: 'expense', normal_balance: 'debit', currency: null, parent: null, tags: [], archived: true },
  ];
}

const ACCOUNT_CLASSES = ['asset', 'liability', 'equity', 'income', 'expense'];
const ACCOUNT_NUMBER = /^[0-9]{2,10}$/;
const TAG = /^[a-z0-9]+(-[a-z0-9]+)*$/;

/** The derived view the server returns: postable, and the balance of the account's own postings. */
function accountView(account: MockAccount) {
  const hasChildren = state.accounts.some((other) => other.parent === account.number);
  const lines = state.entries.flatMap((entry) => entry.lines.filter((line) => line.account_number === account.number));
  const minor = lines.reduce((total, line) => total + BigInt(line.side === 'debit' ? line.base.minor : `-${line.base.minor}`), 0n);
  return {
    ...account,
    is_postable: !hasChildren,
    balances: lines.length === 0 ? [] : [{ minor: minor.toString(), currency: 'EUR' }],
  };
}

/** The balance of one account in EUR, debit-positive; zero when nothing is posted to it. */
function accountBalance(number: string): bigint {
  return state.entries
    .flatMap((entry) => entry.lines.filter((line) => line.account_number === number))
    .reduce((total, line) => total + BigInt(line.side === 'debit' ? line.base.minor : `-${line.base.minor}`), 0n);
}

function findAccount(number: string): MockAccount | undefined {
  return state.accounts.find((account) => account.number === number);
}

/** The accounts routes (accounts.create, get, update, archive, restore). Null when the path is not one. */
function handleAccounts(method: string, path: string, bodyText: string): Reply | null {
  if (method === 'GET' && path === '/v1/accounts') return { status: 200, body: { accounts: state.accounts.map(accountView) } };
  if (method === 'POST' && path === '/v1/accounts') {
    const input = JSON.parse(bodyText || '{}') as { number?: string; name?: string; type?: string; parent?: string; tags?: string[]; currency?: string };
    const number = (input.number ?? '').trim();
    const name = (input.name ?? '').trim();
    if (!ACCOUNT_NUMBER.test(number)) return problemBody(422, 'account-operation-failed', 'Account operation failed', 'account numbers are 2 to 10 ASCII digits');
    if (findAccount(number)) return problemBody(409, 'account-exists', 'Account already exists', `account already exists: ${number}`);
    if (name === '') return problemBody(422, 'account-operation-failed', 'Account operation failed', 'a name is required');
    if (!ACCOUNT_CLASSES.includes(input.type ?? '')) return problemBody(422, 'account-operation-failed', 'Account operation failed', 'unknown account type');
    if (input.parent) {
      const parent = findAccount(input.parent);
      if (!parent) return problemBody(422, 'account-operation-failed', 'Account operation failed', `parent account not found: ${input.parent}`);
      if (parent.account_type !== input.type) return problemBody(422, 'account-operation-failed', 'Account operation failed', 'a child must be in the class of its parent');
      if (accountBalance(parent.number) !== 0n || state.entries.some((e) => e.lines.some((l) => l.account_number === parent.number))) {
        return problemBody(422, 'account-operation-failed', 'Account operation failed', 'the parent already has postings');
      }
    }
    const tags = input.tags ?? [];
    if (tags.some((tag) => !TAG.test(tag)) || new Set(tags).size !== tags.length) {
      return problemBody(422, 'account-operation-failed', 'Account operation failed', 'invalid tags');
    }
    const account: MockAccount = {
      number,
      name_key: `account.custom.${number}`,
      display_name: name,
      account_type: input.type ?? 'asset',
      normal_balance: ['expense', 'asset'].includes(input.type ?? '') ? 'debit' : 'credit',
      currency: input.currency ? input.currency : null,
      parent: input.parent ?? null,
      tags,
      archived: false,
    };
    state.accounts.push(account);
    return { status: 200, body: { account: accountView(account) } };
  }
  const one = /^\/v1\/accounts\/([^/]+)(?:\/(update|archive|restore))?$/.exec(path);
  if (!one) return null;
  const number = decodeURIComponent(one[1]);
  const action = one[2];
  const account = findAccount(number);
  if (method === 'GET' && !action) {
    if (!account) return problemBody(404, 'account-not-found', 'Account not found', `no account ${number}`);
    const activity = state.entries
      .flatMap((entry) =>
        entry.lines
          .filter((line) => line.account_number === number)
          .map((line) => ({
            entry_id: entry.entry_id,
            journal_id: entry.journal_id,
            fiscal_year: entry.fiscal_year,
            number: entry.number,
            posting_date: entry.posting_date,
            description: entry.description,
            minor: (line.side === 'debit' ? BigInt(line.base.minor) : -BigInt(line.base.minor)).toString(),
            currency: line.base.currency,
          })),
      )
      .sort((a, b) => b.posting_date.localeCompare(a.posting_date) || b.number - a.number)
      .slice(0, 20);
    return { status: 200, body: { account: accountView(account), activity } };
  }
  if (method !== 'POST' || !action) return null;
  if (!account) return problemBody(404, 'account-not-found', 'Account not found', `no account ${number}`);
  if (action === 'update') {
    const input = JSON.parse(bodyText || '{}') as { name?: string; type?: string; tags?: string[]; parent?: string; clear_parent?: boolean };
    if (input.name !== undefined) {
      const name = input.name.trim();
      if (name === '') return problemBody(422, 'account-operation-failed', 'Account operation failed', 'a name is required');
      account.display_name = name;
    }
    if (input.type !== undefined && input.type !== account.account_type) {
      if (accountBalance(number) !== 0n) return problemBody(422, 'account-operation-failed', 'Account operation failed', 'the class cannot change once the account has postings');
      account.account_type = input.type;
    }
    if (input.tags !== undefined) {
      if (input.tags.some((tag) => !TAG.test(tag)) || new Set(input.tags).size !== input.tags.length) {
        return problemBody(422, 'account-operation-failed', 'Account operation failed', 'invalid tags');
      }
      account.tags = input.tags;
    }
    if (input.clear_parent) account.parent = null;
    return { status: 200, body: { account: accountView(account) } };
  }
  if (action === 'archive') {
    if (!account.archived && accountBalance(number) !== 0n) {
      return problemBody(422, 'account-operation-failed', 'Account operation failed', `account ${number} has a balance; archiving needs a zero balance`);
    }
    account.archived = true;
    return { status: 200, body: { account: accountView(account) } };
  }
  account.archived = false;
  return { status: 200, body: { account: accountView(account) } };
}

/** The months from `from` to `to` (both YYYY-MM), in order. */
function monthsBetween(from: string, to: string): string[] {
  const out: string[] = [];
  let year = Number(from.slice(0, 4));
  let month = Number(from.slice(5, 7));
  const end = Number(to.slice(0, 4)) * 12 + Number(to.slice(5, 7));
  while (year * 12 + month <= end) {
    out.push(`${year}-${String(month).padStart(2, '0')}`);
    month += 1;
    if (month > 12) {
      month = 1;
      year += 1;
    }
  }
  return out;
}

const DIMENSION_NAMES: Record<string, string> = { 'PRJ-1': 'Website relaunch', 'CC-ADMIN': 'Administration', 'CC-SALES': 'Sales' };
const PERIOD_KEY = /^\d{4}-\d{2}$/;

/**
 * GET /v1/reports/trends: the shape the server answers. Flows are a posting
 * month's income and expense; balances are the position at each month end.
 */
function trendsReply(url: URL): Reply {
  const from = url.searchParams.get('from') ?? '';
  const to = url.searchParams.get('to') ?? '';
  const dimension = url.searchParams.get('dimension');
  if (!PERIOD_KEY.test(from) || !PERIOD_KEY.test(to) || from > to) {
    return problem(422, 'Validation failed', 'from must not be after to');
  }
  const months = monthsBetween(from, to);
  const monthOf = (entry: Entry) => entry.posting_date.slice(0, 7);
  const typeOf = (number: string) => findAccount(number)?.account_type;
  // Signed sum of the lines that pass `keep`: debits minus credits.
  const net = (entries: Entry[], keep: (line: Line) => boolean) =>
    entries.reduce(
      (total, entry) =>
        total + entry.lines.filter(keep).reduce((acc, line) => acc + (line.side === 'debit' ? 1n : -1n) * BigInt(line.base.minor), 0n),
      0n,
    );
  const flow = (period: string, type: 'income' | 'expense') => {
    const posted = state.entries.filter((entry) => monthOf(entry) === period);
    const debitMinusCredit = net(posted, (line) => typeOf(line.account_number) === type);
    return type === 'income' ? -debitMinusCredit : debitMinusCredit;
  };
  const balanceAt = (period: string, accounts: string[]) =>
    net(
      state.entries.filter((entry) => monthOf(entry) <= period),
      (line) => accounts.includes(line.account_number),
    );
  const rows = months.map((period) => {
    const revenue = flow(period, 'income');
    const expenses = flow(period, 'expense');
    return {
      period,
      revenue: money(revenue),
      expenses: money(expenses),
      net_result: money(revenue - expenses),
      cash: money(balanceAt(period, ['1000'])),
      receivables: money(balanceAt(period, ['1200'])),
      payables: money(-balanceAt(period, ['2000'])),
    };
  });

  const inRange = state.entries.filter((entry) => monthOf(entry) >= from && monthOf(entry) <= to);
  const expenseTotals = new Map<string, bigint>();
  for (const entry of inRange) {
    for (const line of entry.lines) {
      if (typeOf(line.account_number) !== 'expense' || line.side !== 'debit') continue;
      expenseTotals.set(line.account_number, (expenseTotals.get(line.account_number) ?? 0n) + BigInt(line.base.minor));
    }
  }
  const totalExpenses = rows.reduce((total, row) => total + BigInt(row.expenses.minor), 0n);
  const top = [...expenseTotals.entries()]
    .sort((a, b) => (a[1] === b[1] ? (a[0] < b[0] ? -1 : 1) : a[1] > b[1] ? -1 : 1))
    .slice(0, 8)
    .map(([account, amount]) => ({
      account,
      name_key: findAccount(account)?.name_key ?? account,
      amount: money(amount),
      share_bps: totalExpenses > 0n ? Number((amount * 10_000n) / totalExpenses) : 0,
    }));

  const by_dimension: { code: string; name: string; revenue: { minor: string; currency: string }; expenses: { minor: string; currency: string } }[] = [];
  if (dimension) {
    const split = new Map<string, { revenue: bigint; expenses: bigint }>();
    for (const entry of inRange) {
      for (const line of entry.lines) {
        const code = line.dimensions?.[dimension];
        const type = typeOf(line.account_number);
        if (!code || (type !== 'income' && type !== 'expense')) continue;
        const slot = split.get(code) ?? { revenue: 0n, expenses: 0n };
        const signed = (line.side === 'debit' ? 1n : -1n) * BigInt(line.base.minor);
        if (type === 'income') slot.revenue -= signed;
        else slot.expenses += signed;
        split.set(code, slot);
      }
    }
    for (const [code, slot] of [...split.entries()].sort((a, b) => (a[0] < b[0] ? -1 : 1))) {
      by_dimension.push({ code, name: DIMENSION_NAMES[code] ?? code, revenue: money(slot.revenue), expenses: money(slot.expenses) });
    }
  }

  const activity = months
    .map((period) => {
      const posted = state.entries.filter((entry) => monthOf(entry) === period);
      return { period, agent: posted.filter((entry) => entry.posted_by === 'agent').length, human: posted.filter((entry) => entry.posted_by !== 'agent').length };
    })
    .filter((row) => row.agent + row.human > 0);

  return {
    status: 200,
    body: { currency: 'EUR', from, to, months: rows, top_expense_accounts: top, dimension: dimension ?? null, by_dimension, activity },
  };
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
  state.accounts = seedAccounts();
  state.entries = options.seed ? seedEntries() : [];
  if (options.extraEntries) state.entries.push(...extraEntriesFor(options.extraEntries));
  state.posts = [];
  state.reversals = [];
  state.person = options.person ?? false;
  resetBills();
  state.approvals = options.person ? approvalSeed() : [];
  state.decisions = [];
  state.bulk = [];
  keyState.keys = [];
  keyCounter = 0;
  resetSettings();
  nextNumber = state.entries.length + 1;
  if (!server) {
    server = http.createServer((req, res) => {
      const chunks: Buffer[] = [];
      req.on('data', (chunk: Buffer) => chunks.push(chunk));
      req.on('end', () => {
        const raw = Buffer.concat(chunks);
        const text = raw.toString('utf8');
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
        // A signed document link serves its file without a credential: the signature is the credential.
        const served = req.method === 'GET' ? servedFile(new URL(req.url ?? '/', API_ORIGIN).pathname) : null;
        if (served) {
          res.writeHead(200, { ...headers, 'Content-Type': served.mime, 'Cache-Control': 'no-store' });
          res.end(served.bytes);
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
        if (!asPerson && req.method === 'POST' && path.startsWith('/v1/accounts')) {
          res.writeHead(403, { ...headers, 'Content-Type': 'application/problem+json' });
          res.end(JSON.stringify(problem(403, 'Human-only', 'this command is human-only').body));
          return;
        }
        if (!asPerson && path.startsWith('/v1/approval-requests')) {
          res.writeHead(403, { ...headers, 'Content-Type': 'application/problem+json' });
          res.end(JSON.stringify(problem(403, 'Forbidden').body));
          return;
        }
        if (path.startsWith('/v1/documents') || path.startsWith('/v1/bills')) {
          const billsReply = handleBills(req.method ?? 'GET', new URL(req.url ?? '/', API_ORIGIN), text, raw, req.headers);
          if (billsReply) {
            res.writeHead(billsReply.status, { ...headers, 'Content-Type': billsReply.problem ? 'application/problem+json' : 'application/json' });
            res.end(JSON.stringify(billsReply.body));
            return;
          }
        }
        // Settings routes (person only; an agent key gets 403 on them).
        const settingsReply = handleSettings(
          req.method ?? 'GET',
          new URL(req.url ?? '/', API_ORIGIN),
          text,
          req.headers.authorization === `Bearer ${API_KEY}`,
        );
        if (settingsReply) {
          res.writeHead(settingsReply.status, { ...headers, 'Content-Type': settingsReply.problem ? 'application/problem+json' : 'application/json' });
          res.end(JSON.stringify(settingsReply.body));
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

/**
 * Puts the API key in sessionStorage before any page script runs (skips the
 * connect screen), with the theme and language to start in.
 */
export async function signIn(page: Page, theme: 'dark' | 'light' = 'dark', locale: 'en' | 'de' | 'id' | 'ar' = 'en') {
  await page.addInitScript(
    ({ key, origin, themeValue, localeValue }) => {
      window.sessionStorage.setItem('ledgers.apiKey', key);
      window.sessionStorage.setItem('ledgers.baseUrl', origin);
      window.localStorage.setItem('ledgers.theme', themeValue);
      window.localStorage.setItem('ledgers.locale', localeValue);
    },
    { key: API_KEY, origin: API_ORIGIN, themeValue: theme, localeValue: locale },
  );
}

/**
 * Starts a person session (no agent key): the screens a signed-in person sees,
 * such as the approvals inbox, which an agent key is refused. Use with
 * mockApi({ person: true }).
 */
export async function signInPerson(page: Page, theme: 'dark' | 'light' = 'dark', locale: 'en' | 'de' | 'id' | 'ar' = 'en') {
  await page.addInitScript(
    ({ themeValue, localeValue }) => {
      window.localStorage.setItem('ledgers.theme', themeValue);
      window.localStorage.setItem('ledgers.locale', localeValue);
    },
    { themeValue: theme, localeValue: locale },
  );
}
