/**
 * The Settings routes of the fake API, behaving like the server's settings
 * commands (crates/ledgers-server/src/dispatch/settings.rs): a person only,
 * numbers that only move forward, a year closes after its months lock, a
 * locked month reopens with a reason, and the audit read filters and pages.
 * mockApi.ts calls handleSettings before its own routes.
 */

export interface SettingsReply {
  status: number;
  body: unknown;
  problem?: boolean;
}

type Period = { period: string; state: 'open' | 'soft_closed' | 'closed' };
type Year = { label: number; starts_on: string; ends_on: string; closed: boolean; closed_at: string | null; periods: Period[] };
type Event = {
  id: string;
  seq: number;
  at: string;
  actor_kind: 'person' | 'agent';
  actor_id: string;
  command: string;
  outcome: 'allowed' | 'needs-approval' | 'denied' | 'error';
  objects: string[];
  reason: string | null;
  request_id: string;
};

const MONTHS = Array.from({ length: 12 }, (_, i) => `2026-${String(i + 1).padStart(2, '0')}`);

export const settingsState = {
  company: {
    name: 'Halden Works GmbH',
    base_currency: 'EUR',
    base_currency_locked: false,
    profile: {
      legal_form: 'GmbH',
      vat_id: 'DE123456789',
      registration_number: 'HRB 12345',
      address_line: 'Hauptstraße 1',
      postal_code: '10115',
      city: 'Berlin',
      country: 'DE',
      contact_email: 'books@halden.example',
    },
  },
  years: [] as Year[],
  journal: { id: 'general', code: 'GN', name: 'General journal', kind: 'general' },
  sequences: [] as { fiscal_year: number; next_number: number; entries: number; editable: boolean }[],
  tax: { small_business: false, vat_basis: 'accrual' as 'accrual' | 'cash' },
  approvals: {
    low_risk_limits: { EUR: '100.00', GBP: '100.00', USD: '100.00' } as Record<string, string>,
    request_ttl_hours: 24,
  },
  status: {
    ok: true,
    extraction: { provider: 'minimax', configured: null as string | null, reason: null as 'missing_key' | 'unknown_provider' | null },
    unavailable: [] as { feature: string; reason: string }[],
    limits: { document_max_bytes: 10 * 1024 * 1024, pdf_max_pages: 50 },
  },
  events: [] as Event[],
};

/** The fixture year: 2026 open, every month open, one journal sequence. */
export function resetSettings(): void {
  settingsState.company.base_currency_locked = false;
  settingsState.years = [
    {
      label: 2026,
      starts_on: '2026-01-01',
      ends_on: '2026-12-31',
      closed: false,
      closed_at: null,
      periods: MONTHS.map((period) => ({ period, state: 'open' as const })),
    },
  ];
  settingsState.sequences = [{ fiscal_year: 2026, next_number: 1, entries: 0, editable: true }];
  settingsState.tax = { small_business: false, vat_basis: 'accrual' };
  settingsState.approvals = { low_risk_limits: { EUR: '100.00', GBP: '100.00', USD: '100.00' }, request_ttl_hours: 24 };
  settingsState.status = {
    ok: true,
    extraction: { provider: 'minimax', configured: null, reason: null },
    unavailable: [],
    limits: { document_max_bytes: 10 * 1024 * 1024, pdf_max_pages: 50 },
  };
  settingsState.events = seedEvents();
}

/** Audit events, newest first. Sequence numbers run down, so the cursor pages them. */
function seedEvents(): Event[] {
  const rows: Omit<Event, 'seq' | 'id' | 'request_id'>[] = [
    { at: '2026-10-22T09:41:00Z', actor_kind: 'person', actor_id: 'person-owner', command: 'company.update', outcome: 'allowed', objects: ['01DEVCOMPANY0000000000000A'], reason: null },
    { at: '2026-10-22T09:12:00Z', actor_kind: 'agent', actor_id: 'lk_live_bank1234', command: 'entries.post', outcome: 'allowed', objects: ['GN', '2026', '1042'], reason: null },
    { at: '2026-10-22T08:58:00Z', actor_kind: 'agent', actor_id: 'lk_live_bank1234', command: 'entries.post', outcome: 'needs-approval', objects: ['GN', '2026'], reason: '1200.00 exceeds per-entry limit 1000.00 for EUR' },
    { at: '2026-10-21T17:03:00Z', actor_kind: 'person', actor_id: 'person-owner', command: 'approval_settings.update', outcome: 'allowed', objects: ['01DEVCOMPANY0000000000000A'], reason: null },
    { at: '2026-10-21T16:20:00Z', actor_kind: 'agent', actor_id: 'lk_live_draft777', command: 'entries.post', outcome: 'denied', objects: [], reason: 'no currency limit for USD' },
    { at: '2026-10-21T10:02:00Z', actor_kind: 'person', actor_id: 'person-owner', command: 'periods.reopen', outcome: 'allowed', objects: ['2026-03'], reason: null },
    { at: '2026-10-20T11:45:00Z', actor_kind: 'person', actor_id: 'person-owner', command: 'tax_settings.update', outcome: 'allowed', objects: ['01DEVCOMPANY0000000000000A'], reason: null },
    { at: '2026-10-20T08:30:00Z', actor_kind: 'agent', actor_id: 'lk_live_bank1234', command: 'entries.post', outcome: 'allowed', objects: ['GN', '2026', '1041'], reason: null },
  ];
  const total = rows.length;
  return rows.map((row, index) => ({
    ...row,
    id: `01AUDIT${String(total - index).padStart(18, '0')}`,
    seq: total - index,
    request_id: `req-${total - index}`,
  }));
}

const problemReply = (status: number, title: string, detail: string): SettingsReply => ({
  status,
  problem: true,
  body: { type: 'https://ledgers.sh/problems/invalid-input', title, status, detail },
});

const ok = (body: unknown): SettingsReply => ({ status: 200, body });

function json(text: string): Record<string, unknown> {
  try {
    return text ? (JSON.parse(text) as Record<string, unknown>) : {};
  } catch {
    return {};
  }
}

function yearView(year: Year) {
  return year;
}

function taxView() {
  const country = settingsState.company.profile.country;
  const german = country === 'DE';
  return {
    small_business: settingsState.tax.small_business,
    vat_basis: settingsState.tax.vat_basis,
    country,
    rate_table: german,
    rates: german
      ? [
          { category: 'standard', percent: '19', valid_from: '2021-01-01', source: '§12 UStG' },
          { category: 'reduced', percent: '7', valid_from: '2021-01-01', source: '§12 UStG' },
        ]
      : [],
    pack: german ? 'de' : '',
    pack_name: german ? 'Germany' : '',
    regime: german ? 'vat' : '',
    user_rates_allowed: false,
    rates_status: german ? 'complete' : '',
    registration_label: german ? 'USt-IdNr.' : '',
    registration_note: german ? 'DE plus nine digits, with the ISO 7064 check digit.' : '',
    returns: german ? [{ id: 'ustva', label: 'Umsatzsteuer-Voranmeldung (UStVA)' }] : [],
    tax_codes: german ? [{ code: '19-U', label: 'Output VAT, standard', kind: 'output_vat', category: 'standard' }] : [],
    user_rates: [],
  };
}

function approvalView() {
  return { ...settingsState.approvals, fresh_sign_in_minutes: 5 };
}

/** The audit read: the filters the app sends, then one page of up to `limit`. */
function auditPage(url: URL): SettingsReply {
  const limit = Math.min(Math.max(Number(url.searchParams.get('limit') ?? 50), 1), 200);
  const before = url.searchParams.get('before_seq');
  const actor = url.searchParams.get('actor_kind');
  const command = url.searchParams.get('command');
  const outcome = url.searchParams.get('outcome');
  const from = url.searchParams.get('from');
  const to = url.searchParams.get('to');
  if (from && to && from > to) return problemReply(422, 'Invalid input', 'from must be on or before to');
  const matches = settingsState.events
    .filter((event) => before === null || event.seq < Number(before))
    .filter((event) => !actor || event.actor_kind === actor)
    .filter((event) => !command || event.command === command)
    .filter((event) => !outcome || event.outcome === outcome)
    .filter((event) => !from || event.at.slice(0, 10) >= from)
    .filter((event) => !to || event.at.slice(0, 10) <= to)
    .sort((a, b) => b.seq - a.seq);
  const page = matches.slice(0, limit);
  const more = matches.length > limit;
  return ok({ events: page, next_before_seq: more ? page[page.length - 1]?.seq ?? null : null });
}

/**
 * Answers a Settings route, or returns null for a path it does not own.
 * `agent` is a request with an agent key: person-only routes answer 403.
 */
export function handleSettings(method: string, url: URL, bodyText: string, agent: boolean): SettingsReply | null {
  const path = url.pathname;
  if (method === 'GET' && path === '/v1/status') return ok(settingsState.status);
  const owned = [
    '/v1/company',
    '/v1/fiscal-years',
    '/v1/fiscal-years/close',
    '/v1/periods/close',
    '/v1/periods/reopen',
    '/v1/numbering',
    '/v1/numbering/next',
    '/v1/tax-settings',
    '/v1/approval-settings',
    '/v1/audit-events',
  ];
  if (!owned.includes(path)) return null;
  if (agent) {
    return { status: 403, problem: true, body: { type: 'https://ledgers.sh/problems/human-only', title: 'Human-only', status: 403, detail: 'this command is human-only' } };
  }
  const body = json(bodyText);

  if (path === '/v1/company') {
    if (method === 'GET') return ok({ ...settingsState.company, profile: { ...settingsState.company.profile } });
    const profile = (body.profile ?? {}) as Record<string, string>;
    const name = String(body.name ?? '').trim();
    if (name === '') return problemReply(422, 'Invalid input', 'Company name is required.');
    const vat = String(profile.vat_id ?? '').replace(/[ .-]/g, '').toUpperCase();
    const country = String(profile.country ?? '').toUpperCase();
    if (vat !== '' && !vat.startsWith(country)) return problemReply(422, 'Invalid input', `A ${country} VAT ID starts with ${country}.`);
    if (vat !== '' && country === 'DE' && !/^DE\d{9}$/.test(vat)) return problemReply(422, 'Invalid input', 'This is not a valid DE VAT ID format.');
    settingsState.company = {
      ...settingsState.company,
      name,
      profile: { ...profile, vat_id: vat, country, legal_form: profile.legal_form ?? '', registration_number: profile.registration_number ?? '', address_line: profile.address_line ?? '', postal_code: profile.postal_code ?? '', city: profile.city ?? '', contact_email: profile.contact_email ?? '' } as typeof settingsState.company.profile,
    };
    settingsState.events.unshift(auditRow('company.update', [settingsState.company.name]));
    return ok({ ...settingsState.company });
  }

  if (path === '/v1/fiscal-years') {
    if (method === 'GET') return ok({ years: settingsState.years.map(yearView) });
    const label = Number(body.label);
    if (settingsState.years.some((year) => year.label === label)) return problemReply(422, 'Account operation failed', `fiscal year already exists: ${label}`);
    const year: Year = {
      label,
      starts_on: `${label}-01-01`,
      ends_on: `${label}-12-31`,
      closed: false,
      closed_at: null,
      periods: Array.from({ length: 12 }, (_, i) => ({ period: `${label}-${String(i + 1).padStart(2, '0')}`, state: 'open' as const })),
    };
    settingsState.years.push(year);
    settingsState.events.unshift(auditRow('fiscal_year.open', [String(label)]));
    return ok(yearView(year));
  }

  if (path === '/v1/fiscal-years/close') {
    const year = settingsState.years.find((item) => item.label === Number(body.label));
    if (!year) return problemReply(422, 'Account operation failed', `fiscal year ${body.label} does not exist`);
    if (year.closed) return problemReply(422, 'Account operation failed', `fiscal year ${year.label} is already closed`);
    // The months before the last close in order; December stays open for the closing entry.
    year.periods.slice(0, -1).forEach((month) => (month.state = 'closed'));
    year.closed = true;
    year.closed_at = '2026-10-23T09:00:00Z';
    settingsState.events.unshift(auditRow('fiscal_year.close', [String(year.label)]));
    return ok({ year: yearView(year), closing_entry_id: null });
  }

  if (path === '/v1/periods/close' || path === '/v1/periods/reopen') {
    const period = String(body.period ?? '');
    const found = settingsState.years.flatMap((year) => year.periods).find((item) => item.period === period);
    if (!found) return problemReply(422, 'Invalid input', `period must be YYYY-MM, got ${JSON.stringify(period)}`);
    if (path === '/v1/periods/close') {
      found.state = 'closed';
      settingsState.events.unshift(auditRow('period.close', [period]));
    } else {
      const reason = String(body.reason ?? '').trim();
      if (reason === '') return problemReply(422, 'Invalid input', 'a reason is required to reopen a period');
      found.state = 'open';
      settingsState.events.unshift({ ...auditRow('period.reopen', [period]), reason });
    }
    return ok({ period, state: found.state });
  }

  if (path === '/v1/numbering') {
    return ok({
      journals: [{ ...settingsState.journal, sequences: settingsState.sequences.map((seq) => ({ ...seq })) }],
    });
  }

  if (path === '/v1/numbering/next') {
    const sequence = settingsState.sequences.find((item) => item.fiscal_year === Number(body.fiscal_year));
    if (!sequence) return problemReply(422, 'Invalid input', 'fiscal year is not open');
    const next = Number(body.next_number);
    if (sequence.entries > 0) return problemReply(422, 'Invalid input', 'numbers have already been issued in this journal and year; moving them would leave a gap');
    if (!(next > sequence.next_number)) return problemReply(422, 'Invalid input', `the next number can only move forward; it is ${sequence.next_number} now`);
    sequence.next_number = next;
    settingsState.events.unshift(auditRow('numbering.set_next', [settingsState.journal.id, String(sequence.fiscal_year)]));
    return ok({ journal_id: settingsState.journal.id, fiscal_year: sequence.fiscal_year, next_number: next });
  }

  if (path === '/v1/tax-settings') {
    if (method === 'GET') return ok(taxView());
    const basis = String(body.vat_basis ?? '');
    if (basis !== 'accrual' && basis !== 'cash') return problemReply(422, 'Invalid input', `unknown vat basis ${JSON.stringify(basis)}, expected accrual|cash`);
    settingsState.tax = { small_business: Boolean(body.small_business), vat_basis: basis };
    settingsState.events.unshift(auditRow('tax.set_settings', [settingsState.company.profile.country]));
    return ok(taxView());
  }

  if (path === '/v1/approval-settings') {
    if (method === 'GET') return ok(approvalView());
    const ttl = Number(body.request_ttl_hours);
    if (!Number.isInteger(ttl) || ttl < 1 || ttl > 168) return problemReply(422, 'Invalid input', 'Requests expire after 1 to 168 hours.');
    settingsState.approvals = { low_risk_limits: body.low_risk_limits as Record<string, string>, request_ttl_hours: ttl };
    settingsState.events.unshift(auditRow('approval_settings.update', ['company']));
    return ok(approvalView());
  }

  if (path === '/v1/audit-events') return auditPage(url);
  return null;
}

/** An audit row for a change the person just made. */
function auditRow(command: string, objects: string[]): Event {
  const seq = settingsState.events.length ? Math.max(...settingsState.events.map((event) => event.seq)) + 1 : 1;
  return {
    id: `01AUDIT${String(seq).padStart(18, '0')}`,
    seq,
    at: '2026-10-23T09:00:00Z',
    actor_kind: 'person',
    actor_id: 'person-owner',
    command,
    outcome: 'allowed',
    objects,
    reason: null,
    request_id: `req-${seq}`,
  };
}
