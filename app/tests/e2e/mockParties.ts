/**
 * The fake customers and vendors API (issue #44) for the Playwright specs. It
 * answers the party commands with the server's rules: the field checks of
 * parties.validate, the refusals of create, update, merge and check-vat, and
 * the human-only bank change. PARTIES_SHOTS and the specs read `partiesState`.
 */
import { peopleState } from './mockPeople';
import { billsMock } from './mockBills';

export interface MockParty {
  id: string;
  name: string;
  vat_id: string | null;
  iban: string | null;
  bic: string | null;
  roles: 'customer' | 'vendor' | 'both';
  merged_into: string | null;
  needs_review: boolean;
  personal_data: boolean;
  tax_number: string | null;
  language: string | null;
  currency: string | null;
  payment_terms_days: number | null;
  default_account: string | null;
  default_tax_code: string | null;
  addresses: { kind: string | null; line1: string; line2: string | null; postal_code: string | null; city: string; country: string }[];
  contacts: { name: string; email: string | null; phone: string | null; role: string | null; primary: boolean }[];
  trading_name: string | null;
  registration_number: string | null;
  website: string | null;
  email: string | null;
  phone: string | null;
  default_unit: string | null;
  tags: string[];
  notes: string | null;
  tax_ids: { country: string; kind: 'vat' | 'tax' | 'other'; value: string }[];
  logo_document_id: string | null;
  open_items: MockOpenItem[];
  latest_vies: { valid: boolean; consultation_number: string | null; checked_at: string } | null;
  bank_changes: { id: string; before_iban: string | null; before_bic: string | null; after_iban: string | null; after_bic: string | null; changed_by: string; changed_at: string; reason: string | null }[];
  bill_count: number;
}

/** One open item of a party, as the open-items read returns it. */
export interface MockOpenItem {
  id: string;
  ledger: 'ar' | 'ap';
  kind: 'invoice' | 'credit-note' | 'bill' | 'on-account';
  document: string | null;
  posting_date: string;
  due_date: string | null;
  currency: string;
  original_minor: number;
  remaining_minor: number;
}

export interface MockPartiesState {
  parties: MockParty[];
  /** Whether VAT checks are on for this (fake) server. */
  viesEnabled: boolean;
  /** Every party request as `METHOD path`, and the JSON bodies sent to create, update and merge. */
  requests: string[];
  bodies: { path: string; body: unknown }[];
  /** When set, the next merge is refused with this message (a conflict the server reports). */
  refuseMerge: string | null;
}

function freshState(): MockPartiesState {
  const now = '2026-10-20T09:00:00Z';
  const base = (over: Partial<MockParty> & Pick<MockParty, 'id' | 'name'>): MockParty => ({
    trading_name: null,
    registration_number: null,
    website: null,
    email: null,
    phone: null,
    default_unit: null,
    tags: [],
    notes: null,
    tax_ids: [],
    logo_document_id: null,
    open_items: [],
    vat_id: null,
    iban: null,
    bic: null,
    roles: 'vendor',
    merged_into: null,
    needs_review: false,
    personal_data: false,
    tax_number: null,
    language: null,
    currency: null,
    payment_terms_days: null,
    default_account: null,
    default_tax_code: null,
    addresses: [],
    contacts: [],
    latest_vies: null,
    bank_changes: [],
    bill_count: 0,
    ...over,
  });
  return {
    viesEnabled: true,
    requests: [],
    bodies: [],
    refuseMerge: null,
    parties: [
      base({
        id: 'party-halden',
        name: 'Halden Works GmbH',
        vat_id: 'DE136695976',
        iban: 'DE89370400440532013000',
        bic: 'COBADEFFXXX',
        roles: 'vendor',
        payment_terms_days: 30,
        currency: 'EUR',
        addresses: [{ kind: 'billing', line1: 'Musterstrasse 1', line2: null, postal_code: '10115', city: 'Berlin', country: 'DE' }],
        contacts: [{ name: 'Lea Hartmann', email: 'lea@halden.example', phone: '+49 30 1234567', role: 'Accounts', primary: true }],
        trading_name: 'Halden',
        website: 'https://halden.example',
        email: 'billing@halden.example',
        phone: '+49 30 1234567',
        default_unit: 'berlin-1',
        tags: ['wholesale', 'berlin'],
        notes: 'Pays by bank transfer on the 30th.',
        tax_ids: [{ country: 'NL', kind: 'tax', value: '123456789' }],
        open_items: [
          { id: 'oi-1', ledger: 'ap', kind: 'bill', document: 'HW-2026-114', posting_date: '2026-10-02', due_date: '2026-11-01', currency: 'EUR', original_minor: 184_000, remaining_minor: 184_000 },
          { id: 'oi-2', ledger: 'ap', kind: 'bill', document: 'HW-2026-120', posting_date: '2026-10-12', due_date: '2026-11-11', currency: 'EUR', original_minor: 58_000, remaining_minor: 58_000 },
        ],
        latest_vies: { valid: true, consultation_number: 'WAPIAAAAY1', checked_at: now },
        bank_changes: [
          {
            id: 'chg-1',
            before_iban: null,
            before_bic: null,
            after_iban: 'DE89370400440532013000',
            after_bic: 'COBADEFFXXX',
            changed_by: 'person_01',
            changed_at: '2026-09-02T10:00:00Z',
            reason: 'Set up from the signed contract',
          },
        ],
        bill_count: 2,
        needs_review: false,
      }),
      base({ id: 'party-acme', name: 'Acme GmbH', vat_id: 'DE811907153', roles: 'customer', currency: 'EUR' }),
      base({ id: 'party-lumen', name: 'Lumen Logistik AG', vat_id: 'CHE123456789', roles: 'both', needs_review: true }),
      base({ id: 'party-tovar', name: 'Tovar Studio', roles: 'vendor', needs_review: true }),
      base({ id: 'party-old', name: 'Halden Works (old)', vat_id: 'DE000000001', merged_into: 'party-halden' }),
    ],
  };
}

export const partiesState: MockPartiesState = freshState();

export function resetParties(): void {
  Object.assign(partiesState, freshState());
}

interface PartiesReply {
  status: number;
  body?: unknown;
  problem?: boolean;
}

function problem(status: number, title: string, detail: string, slug = 'invalid-input'): PartiesReply {
  return { status, problem: true, body: { type: `https://ledgers.sh/problems/${slug}`, title, status, detail } };
}

/** A party problem: the slug the app words in its own language, the server's detail as the fallback. */
function refusal(slug: string, detail: string, status = 422): PartiesReply {
  return problem(status, slug, detail, slug);
}

function parse(text: string): Record<string, unknown> {
  try {
    return JSON.parse(text || '{}') as Record<string, unknown>;
  } catch {
    return {};
  }
}

/** The field checks the server runs (the country packs are approximated here by shape). */
function fieldErrors(input: Record<string, unknown>, selfId?: string): { field: string; message: string }[] {
  const errors: { field: string; message: string }[] = [];
  if (typeof input.name === 'string' && input.name.trim() === '') errors.push({ field: 'name', message: 'the party needs a name' });
  if (typeof input.vat_id === 'string' && input.vat_id.trim() !== '') {
    const vat = input.vat_id.replace(/[\s.-]+/g, '').toUpperCase();
    const country = vat.slice(0, 2);
    const body = vat.slice(2);
    if (!/^[A-Z]{2}$/.test(country)) errors.push({ field: 'vat_id', message: 'a VAT ID starts with its two-letter country code' });
    else if (country === 'DE' && !/^\d{9}$/.test(body)) errors.push({ field: 'vat_id', message: 'a German VAT ID is DE followed by nine digits' });
    else {
      const owner = partiesState.parties.find((p) => p.vat_id === vat && p.id !== selfId);
      if (owner) errors.push({ field: 'vat_id', message: `party ${owner.id} already has this VAT ID` });
    }
  }
  if (typeof input.iban === 'string' && input.iban.trim() !== '') {
    const iban = input.iban.replace(/\s+/g, '').toUpperCase();
    if (!/^[A-Z]{2}\d{2}[A-Z0-9]{11,30}$/.test(iban)) errors.push({ field: 'iban', message: 'this IBAN has the wrong shape or length' });
  }
  if (typeof input.bic === 'string' && input.bic.trim() !== '' && !/^[A-Za-z0-9]{8}([A-Za-z0-9]{3})?$/.test(input.bic.trim())) {
    errors.push({ field: 'bic', message: 'a BIC is 8 or 11 letters and digits' });
  }
  if (typeof input.roles === 'string' && !['customer', 'vendor', 'both'].includes(input.roles)) {
    errors.push({ field: 'roles', message: "roles must be 'customer', 'vendor', or 'both'" });
  }
  if (typeof input.payment_terms_days === 'number' && input.payment_terms_days < 0) {
    errors.push({ field: 'payment_terms_days', message: 'payment_terms_days must be >= 0' });
  }
  if (Array.isArray(input.addresses)) {
    input.addresses.forEach((a: Record<string, unknown>, index: number) => {
      if (!String(a.line1 ?? '').trim()) errors.push({ field: 'addresses', message: `address ${index + 1} needs its first line` });
      else if (!String(a.city ?? '').trim()) errors.push({ field: 'addresses', message: `address ${index + 1} needs a city` });
      else if (!/^[A-Za-z]{2}$/.test(String(a.country ?? '').trim())) errors.push({ field: 'addresses', message: `address ${index + 1} needs a two-letter country code` });
    });
  }
  if (Array.isArray(input.contacts)) {
    input.contacts.forEach((c: Record<string, unknown>, index: number) => {
      if (!String(c.name ?? '').trim()) errors.push({ field: 'contacts', message: `contact ${index + 1} needs a name` });
      else if (typeof c.email === 'string' && c.email !== '' && !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(c.email)) {
        errors.push({ field: 'contacts', message: `contact ${index + 1} has an email address that does not look right` });
      }
    });
    if (input.contacts.filter((c: Record<string, unknown>) => c.primary === true).length > 1) {
      errors.push({ field: 'contacts', message: 'only one contact can be the primary contact' });
    }
  }
  if (typeof input.website === 'string' && input.website.trim() !== '' && !/^https?:\/\/[^\s/]+\.[^\s/]+/.test(input.website.trim())) {
    errors.push({ field: 'website', message: 'a website starts with http:// or https:// and names a domain' });
  }
  if (typeof input.email === 'string' && input.email.trim() !== '' && !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(input.email.trim())) {
    errors.push({ field: 'email', message: 'the email address does not look right' });
  }
  return errors;
}

/** The problem slug the server names a refused field with (the app words each one in its language). */
function slugOf(field: string, message: string): string {
  if (field === 'contacts') return message.includes('primary') ? 'party-contact-primary-duplicate' : 'party-contact-invalid';
  if (field === 'vat_id' && message.includes('already has this VAT ID')) return 'party-vat-taken';
  const slugs: Record<string, string> = {
    name: 'party-name-required',
    vat_id: 'party-vat-invalid',
    iban: 'party-iban-invalid',
    bic: 'party-bic-invalid',
    roles: 'party-roles-invalid',
    payment_terms_days: 'party-payment-terms-invalid',
    addresses: 'party-address-invalid',
    website: 'party-website-invalid',
    email: 'party-email-invalid',
  };
  return slugs[field] ?? 'invalid-input';
}

/** A party as the list and the write replies carry it (the history and the bill count are on the detail). */
function view(party: MockParty): Record<string, unknown> {
  return { ...party, bank_changes: undefined, bill_count: undefined };
}

function detail(party: MockParty) {
  return {
    party: { ...view(party), open_items: undefined, possible_duplicates: partiesState.parties.filter((p) => p !== party && !p.merged_into && p.name.split(' ')[0] === party.name.split(' ')[0]).map((p) => p.id) },
    bank_changes: [...party.bank_changes].sort((a, b) => (a.changed_at < b.changed_at ? 1 : -1)),
    bill_count: party.bill_count,
    vies_enabled: partiesState.viesEnabled,
    recent_bills: [],
    recent_payments: [],
  };
}

/** The open items and their totals, as the subledger read answers them. */
function openItems(party: MockParty) {
  const balances: { ledger: 'ar' | 'ap'; currency: string; remaining_minor: number }[] = [];
  for (const item of party.open_items) {
    const found = balances.find((b) => b.ledger === item.ledger && b.currency === item.currency);
    if (found) found.remaining_minor += item.remaining_minor;
    else balances.push({ ledger: item.ledger, currency: item.currency, remaining_minor: item.remaining_minor });
  }
  return { open_items: party.open_items, balances };
}

/** The file's cells, split on commas, quotes kept simple: enough for the rows the specs write. */
function csvRows(text: string): string[][] {
  return text
    .split(/\r?\n/)
    .filter((line) => line.trim() !== '')
    .map((line) => line.split(','));
}

/** Mirrors the server: a cell starting with = + - @ tab CR or an apostrophe gets one apostrophe in front. */
function guardFormula(value: string): string {
  return /^['=+\-@\t\r]/.test(value) ? `'${value}` : value;
}

/** Mirrors the server: drops exactly one leading apostrophe when a trigger or another apostrophe follows it. */
function unguardFormula(value: string): string {
  return /^'['=+\-@\t\r]/.test(value) ? value.slice(1) : value;
}

const CSV_COLUMNS = ['id', 'name', 'trading_name', 'roles', 'vat_id', 'tax_ids', 'tax_number', 'registration_number', 'website', 'email', 'phone', 'currency', 'language', 'payment_terms_days', 'default_account', 'default_tax_code', 'default_unit', 'tags', 'notes', 'personal_data', 'addresses', 'contacts', 'iban', 'bic', 'bank_reason'];

/** The export: every live party, in name order, in the columns the import reads. */
function exportCsv(): string {
  const rows = partiesState.parties
    .filter((p) => !p.merged_into)
    .sort((a, b) => a.name.localeCompare(b.name))
    .map((p) => [p.id, p.name, p.trading_name ?? '', p.roles, p.vat_id ?? '', '', p.tax_number ?? '', p.registration_number ?? '', p.website ?? '', p.email ?? '', p.phone ?? '', p.currency ?? '', p.language ?? '', p.payment_terms_days === null ? '' : String(p.payment_terms_days), p.default_account ?? '', p.default_tax_code ?? '', p.default_unit ?? '', p.tags.join(';'), p.notes ?? '', p.personal_data ? 'true' : 'false', '', '', p.iban ?? '', p.bic ?? '', '']);
  return [CSV_COLUMNS.join(','), ...rows.map((row) => row.map((cell) => (/[,"\n]/.test(guardFormula(cell)) ? `"${guardFormula(cell).replace(/"/g, '""')}"` : guardFormula(cell))).join(','))].join('\n') + '\n';
}

/** Checks a CSV file row by row, the way the server does: what each row would do, and its refusals. */
function checkCsv(text: string, asPerson: boolean) {
  const [header, ...body] = csvRows(text);
  const columns = header ?? [];
  const rows = body.map((cells, index) => {
    const cell = (name: string) => (columns.indexOf(name) === -1 ? undefined : unguardFormula((cells[columns.indexOf(name)] ?? '').trim()));
    const id = cell('id') ?? '';
    const existing = partiesState.parties.find((p) => p.id === id && !p.merged_into);
    const errors: { slug: string; field: string; message: string }[] = [];
    if (!cell('name')) errors.push({ slug: 'party-name-required', field: 'name', message: 'the party needs a name' });
    if (cell('vat_id')?.toUpperCase().startsWith('XX')) errors.push({ slug: 'party-vat-invalid', field: 'vat_id', message: 'this VAT ID does not pass its country check' });
    if (cell('email') && !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(cell('email') ?? '')) errors.push({ slug: 'party-email-invalid', field: 'email', message: 'the email address does not look right' });
    if ((cell('iban') ?? '') !== '' && !asPerson) errors.push({ slug: 'party-bank-needs-person', field: 'iban', message: 'bank details change only by a person, never by an agent key' });
    if ((cell('iban') ?? '') !== '' && asPerson && !cell('bank_reason')) errors.push({ slug: 'party-bank-reason-required', field: 'bank_reason', message: 'a bank change needs a reason, kept in the history' });
    return { line: index + 2, action: errors.length > 0 ? ('error' as const) : existing ? ('update' as const) : ('create' as const), id: id || `party_csv_${index + 1}`, errors };
  });
  const errors = rows.filter((r) => r.errors.length > 0).length;
  return {
    valid: errors === 0,
    creates: rows.filter((r) => r.action === 'create').length,
    updates: rows.filter((r) => r.action === 'update').length,
    errors,
    rows,
  };
}

/** A party with every directory field empty, for the routes that create one. */
function blankParty(id: string, name: string): MockParty {
  return {
    id,
    name,
    vat_id: null,
    iban: null,
    bic: null,
    roles: 'vendor',
    merged_into: null,
    needs_review: false,
    personal_data: false,
    tax_number: null,
    language: null,
    currency: null,
    payment_terms_days: null,
    default_account: null,
    default_tax_code: null,
    addresses: [],
    contacts: [],
    latest_vies: null,
    bank_changes: [],
    bill_count: 0,
    trading_name: null,
    registration_number: null,
    website: null,
    email: null,
    phone: null,
    default_unit: null,
    tags: [],
    notes: null,
    tax_ids: [],
    logo_document_id: null,
    open_items: [],
  };
}

/** The party routes. Returns null for paths that are not parties'. */
export function handleParties(method: string, url: URL, bodyText: string, asPerson: boolean): PartiesReply | null {
  const path = url.pathname;
  if (!path.startsWith('/v1/parties')) return null;
  partiesState.requests.push(`${method} ${path}`);
  const input = parse(bodyText);
  const canManage = asPerson && (peopleState.role === 'owner' || peopleState.role === 'admin');

  if (method === 'GET' && path === '/v1/parties') {
    const q = url.searchParams.get('q')?.trim().toLowerCase() ?? '';
    const roles = url.searchParams.get('roles');
    const review = url.searchParams.get('needs_review');
    const limit = Math.min(200, Number(url.searchParams.get('limit') ?? 50));
    const offset = Number(url.searchParams.get('offset') ?? 0);
    const live = partiesState.parties.filter((p) => !p.merged_into);
    const matching = live.filter(
      (p) =>
        (q === '' || p.name.toLowerCase().includes(q) || (p.vat_id ?? '').toLowerCase().includes(q) || (p.iban ?? '').toLowerCase().includes(q.replace(/\s+/g, ''))) &&
        (!roles || p.roles === roles) &&
        (review === null || String(p.needs_review) === review),
    );
    const tag = url.searchParams.get('tag')?.trim().toLowerCase() ?? '';
    const tagged = matching.filter((p) => tag === '' || p.tags.includes(tag));
    return { status: 200, body: { parties: tagged.slice(offset, offset + limit).map((p) => ({ ...view(p), possible_duplicates: [] })), total: tagged.length } };
  }
  if (method === 'GET' && path === '/v1/parties/export') {
    return { status: 200, body: { csv: exportCsv(), rows: partiesState.parties.filter((p) => !p.merged_into).length } };
  }
  if (method === 'POST' && path === '/v1/parties/import/check') {
    const csv = String(input.csv ?? '');
    return { status: 200, body: checkCsv(csv, asPerson) };
  }
  if (method === 'POST' && path === '/v1/parties/import') {
    const csv = String(input.csv ?? '');
    const report = checkCsv(csv, asPerson);
    if (!report.valid) return refusal('party-import-invalid', 'Some rows have errors. Nothing was imported.');
    partiesState.bodies.push({ path, body: input });
    let created = 0;
    let updated = 0;
    const ids: string[] = [];
    for (const row of report.rows) {
      ids.push(row.id);
      if (row.action === 'update') updated += 1;
      else {
        created += 1;
        partiesState.parties.push({ ...blankParty(row.id, row.id), name: row.id, roles: 'vendor' });
      }
    }
    return { status: 200, body: { created, updated, ids } };
  }
  if (method === 'GET' && path === '/v1/parties/balances') {
    const balances = partiesState.parties
      .filter((p) => p.id === 'party-halden' && !p.merged_into)
      .map((p) => ({ party_id: p.id, currency: 'EUR', payable_minor: 184_000, open_bills: 2 }));
    return { status: 200, body: { balances } };
  }
  if (method === 'POST' && path === '/v1/parties/validate') {
    const errors = fieldErrors(input, typeof input.id === 'string' ? input.id : undefined).map((e) => ({ ...e, slug: slugOf(e.field, e.message) }));
    return { status: 200, body: { valid: errors.length === 0, errors } };
  }
  if (method === 'POST' && path === '/v1/parties') {
    partiesState.bodies.push({ path, body: input });
    const errors = fieldErrors(input);
    if (errors.length > 0) return refusal(slugOf(errors[0].field, errors[0].message), errors[0].message);
    const id = `party-new-${partiesState.parties.length + 1}`;
    const created: MockParty = {
      ...blankParty(id, String(input.name ?? '').trim()),
      id,
      name: String(input.name ?? '').trim(),
      vat_id: typeof input.vat_id === 'string' && input.vat_id !== '' ? input.vat_id.replace(/[\s.-]+/g, '').toUpperCase() : null,
      iban: null,
      bic: null,
      roles: (input.roles as MockParty['roles']) ?? 'vendor',
      merged_into: null,
      needs_review: false,
      personal_data: Boolean(input.personal_data),
      tax_number: (input.tax_number as string) || null,
      language: (input.language as string) || null,
      currency: (input.currency as string) || null,
      payment_terms_days: (input.payment_terms_days as number) ?? null,
      default_account: (input.default_account as string) || null,
      default_tax_code: (input.default_tax_code as string) || null,
      addresses: (input.addresses as MockParty['addresses']) ?? [],
      contacts: (input.contacts as MockParty['contacts']) ?? [],
      trading_name: (input.trading_name as string) || null,
      registration_number: (input.registration_number as string) || null,
      website: (input.website as string) || null,
      email: (input.email as string) || null,
      phone: (input.phone as string) || null,
      default_unit: (input.default_unit as string) || null,
      tags: (input.tags as string[]) ?? [],
      notes: (input.notes as string) || null,
      tax_ids: (input.tax_ids as MockParty['tax_ids']) ?? [],
      latest_vies: null,
      bank_changes: [],
      bill_count: 0,
      open_items: [],
    };
    partiesState.parties.push(created);
    return { status: 200, body: { party: view(created) } };
  }

  const match = /^\/v1\/parties\/([^/]+)(?:\/(update|check-vat|set-bank|merge|set-logo|open-items))?$/.exec(path);
  if (match && match[1] !== 'validate' && match[1] !== 'balances') {
    const id = decodeURIComponent(match[1]);
    const action = match[2];
    const party = partiesState.parties.find((p) => p.id === id);
    if (!party) return problem(422, 'Invalid input', 'party not found');

    if (method === 'GET' && !action) return { status: 200, body: detail(party) };
    if (method === 'GET' && action === 'open-items') return { status: 200, body: openItems(party) };
    if (method === 'POST' && action === 'set-logo') {
      partiesState.bodies.push({ path, body: input });
      const doc = typeof input.logo_document_id === 'string' && input.logo_document_id.trim() !== '' ? input.logo_document_id.trim() : null;
      if (doc) {
        const stored = billsMock.documents.find((d) => d.id === doc);
        if (!stored) return refusal('party-logo-not-stored', 'the logo must be an image this company has uploaded');
        if (stored.mime !== 'image/png' && stored.mime !== 'image/jpeg') return refusal('party-logo-not-image', 'a logo must be a PNG or JPEG image');
        if (stored.size_bytes > 2 * 1024 * 1024) return refusal('party-logo-too-large', 'a logo may be at most 2 MiB');
      }
      party.logo_document_id = doc;
      return { status: 200, body: { party: view(party) } };
    }

    if (method === 'POST' && action === 'update') {
      partiesState.bodies.push({ path, body: input });
      if (party.merged_into) return problem(422, 'Invalid input', 'party is merged and cannot be updated');
      const errors = fieldErrors(input, id);
      if (errors.length > 0) return refusal(slugOf(errors[0].field, errors[0].message), errors[0].message);
      if (typeof input.name === 'string') party.name = input.name.trim();
      if (typeof input.vat_id === 'string') party.vat_id = input.vat_id === '' ? null : input.vat_id.replace(/[\s.-]+/g, '').toUpperCase();
      if (typeof input.roles === 'string') party.roles = input.roles as MockParty['roles'];
      if (typeof input.payment_terms_days === 'number') party.payment_terms_days = input.payment_terms_days;
      if (Array.isArray(input.addresses)) party.addresses = input.addresses as MockParty['addresses'];
      if (Array.isArray(input.contacts)) party.contacts = input.contacts as MockParty['contacts'];
      if (typeof input.website === 'string') party.website = input.website === '' ? null : input.website;
      if (typeof input.email === 'string') party.email = input.email === '' ? null : input.email;
      if (typeof input.phone === 'string') party.phone = input.phone === '' ? null : input.phone;
      if (typeof input.registration_number === 'string') party.registration_number = input.registration_number === '' ? null : input.registration_number;
      if (typeof input.trading_name === 'string') party.trading_name = input.trading_name === '' ? null : input.trading_name;
      if (typeof input.default_unit === 'string') party.default_unit = input.default_unit === '' ? null : input.default_unit;
      if (typeof input.notes === 'string') party.notes = input.notes === '' ? null : input.notes;
      if (Array.isArray(input.tags)) party.tags = input.tags as string[];
      if (Array.isArray(input.tax_ids)) party.tax_ids = input.tax_ids as MockParty['tax_ids'];
      return { status: 200, body: { party: view(party) } };
    }
    if (method === 'POST' && action === 'check-vat') {
      if (!partiesState.viesEnabled) return problem(422, 'Invalid input', 'VAT checks are off on this server (LEDGERS_VIES is not 1)');
      const valid = (party.vat_id ?? '').startsWith('DE');
      party.latest_vies = { valid, consultation_number: 'WAPIAAAAY2', checked_at: '2026-10-22T08:00:00Z' };
      return { status: 200, body: { vies_result: { valid, name: party.name, address: null, consultation_number: 'WAPIAAAAY2', request_date: '2026-10-22' } } };
    }
    if (method === 'POST' && action === 'set-bank') {
      partiesState.bodies.push({ path, body: input });
      if (!asPerson) return problem(403, 'Human-only', 'this command is human-only');
      if (!canManage) return problem(403, 'Forbidden', 'only an owner or an admin changes bank details');
      if (party.merged_into) return problem(422, 'Invalid input', 'party is merged and cannot be changed');
      const reason = String(input.reason ?? '').trim();
      if (reason === '') return problem(422, 'Invalid input', 'a bank change needs a reason');
      const iban = String(input.iban ?? '').replace(/\s+/g, '').toUpperCase();
      if (!/^[A-Z]{2}\d{2}[A-Z0-9]{11,30}$/.test(iban)) return problem(422, 'Invalid input', 'this IBAN has the wrong shape or length');
      party.bank_changes.push({
        id: `chg-${party.bank_changes.length + 1}`,
        before_iban: party.iban,
        before_bic: party.bic,
        after_iban: iban,
        after_bic: (input.bic as string) || null,
        changed_by: 'person_01',
        changed_at: '2026-10-22T09:30:00Z',
        reason,
      });
      party.iban = iban;
      party.bic = (input.bic as string) || null;
      return { status: 200, body: { party: view(party) } };
    }
    if (method === 'POST' && action === 'merge') {
      const from = String(input.from ?? '');
      partiesState.bodies.push({ path, body: input });
      if (!asPerson) return problem(403, 'Human-only', 'this command is human-only');
      if (!canManage) return problem(403, 'Forbidden', 'only an owner or an admin merges parties');
      if (partiesState.refuseMerge) return problem(422, 'Invalid input', partiesState.refuseMerge);
      const source = partiesState.parties.find((p) => p.id === from);
      if (!source || source.id === party.id) return problem(422, 'Invalid input', 'cannot merge a party into itself');
      if (party.merged_into) return problem(422, 'Invalid input', 'survivor party is already merged');
      if (source.merged_into) return problem(422, 'Invalid input', 'source party is already merged');
      source.merged_into = party.id;
      party.bill_count += source.bill_count;
      source.bill_count = 0;
      return { status: 200, body: { party: view(party) } };
    }
  }
  return problem(404, 'Not found', `no party route ${method} ${path}`);
}
