/**
 * The fake customers and vendors API (issue #44) for the Playwright specs. It
 * answers the party commands with the server's rules: the field checks of
 * parties.validate, the refusals of create, update, merge and check-vat, and
 * the human-only bank change. PARTIES_SHOTS and the specs read `partiesState`.
 */
import { peopleState } from './mockPeople';

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
  contacts: { name: string; email: string | null; phone: string | null; role: string | null }[];
  latest_vies: { valid: boolean; consultation_number: string | null; checked_at: string } | null;
  bank_changes: { id: string; before_iban: string | null; before_bic: string | null; after_iban: string | null; after_bic: string | null; changed_by: string; changed_at: string; reason: string | null }[];
  bill_count: number;
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
        contacts: [{ name: 'Lea Hartmann', email: 'lea@halden.example', phone: '+49 30 1234567', role: 'Accounts' }],
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

function problem(status: number, title: string, detail: string): PartiesReply {
  return { status, problem: true, body: { type: 'https://ledgers.sh/problems/invalid-input', title, status, detail } };
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
  }
  return errors;
}

/** A party as the list and the write replies carry it (the history and the bill count are on the detail). */
function view(party: MockParty): Record<string, unknown> {
  return { ...party, bank_changes: undefined, bill_count: undefined };
}

function detail(party: MockParty) {
  return {
    party: { ...view(party), possible_duplicates: partiesState.parties.filter((p) => p !== party && !p.merged_into && p.name.split(' ')[0] === party.name.split(' ')[0]).map((p) => p.id) },
    bank_changes: [...party.bank_changes].sort((a, b) => (a.changed_at < b.changed_at ? 1 : -1)),
    bill_count: party.bill_count,
    vies_enabled: partiesState.viesEnabled,
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
    return { status: 200, body: { parties: matching.slice(offset, offset + limit).map((p) => ({ ...view(p), possible_duplicates: [] })), total: matching.length } };
  }
  if (method === 'GET' && path === '/v1/parties/balances') {
    const balances = partiesState.parties
      .filter((p) => p.id === 'party-halden' && !p.merged_into)
      .map((p) => ({ party_id: p.id, currency: 'EUR', payable_minor: 184_000, open_bills: 2 }));
    return { status: 200, body: { balances } };
  }
  if (method === 'POST' && path === '/v1/parties/validate') {
    const errors = fieldErrors(input, typeof input.id === 'string' ? input.id : undefined);
    return { status: 200, body: { valid: errors.length === 0, errors } };
  }
  if (method === 'POST' && path === '/v1/parties') {
    partiesState.bodies.push({ path, body: input });
    const errors = fieldErrors(input);
    if (errors.length > 0) return problem(422, 'Invalid input', errors[0].message);
    const id = `party-new-${partiesState.parties.length + 1}`;
    const created: MockParty = {
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
      latest_vies: null,
      bank_changes: [],
      bill_count: 0,
    };
    partiesState.parties.push(created);
    return { status: 200, body: { party: view(created) } };
  }

  const match = /^\/v1\/parties\/([^/]+)(?:\/(update|check-vat|set-bank|merge))?$/.exec(path);
  if (match && match[1] !== 'validate' && match[1] !== 'balances') {
    const id = decodeURIComponent(match[1]);
    const action = match[2];
    const party = partiesState.parties.find((p) => p.id === id);
    if (!party) return problem(422, 'Invalid input', 'party not found');

    if (method === 'GET' && !action) return { status: 200, body: detail(party) };

    if (method === 'POST' && action === 'update') {
      partiesState.bodies.push({ path, body: input });
      if (party.merged_into) return problem(422, 'Invalid input', 'party is merged and cannot be updated');
      const errors = fieldErrors(input, id);
      if (errors.length > 0) return problem(422, 'Invalid input', errors[0].message);
      if (typeof input.name === 'string') party.name = input.name.trim();
      if (typeof input.vat_id === 'string') party.vat_id = input.vat_id === '' ? null : input.vat_id.replace(/[\s.-]+/g, '').toUpperCase();
      if (typeof input.roles === 'string') party.roles = input.roles as MockParty['roles'];
      if (typeof input.payment_terms_days === 'number') party.payment_terms_days = input.payment_terms_days;
      if (Array.isArray(input.addresses)) party.addresses = input.addresses as MockParty['addresses'];
      if (Array.isArray(input.contacts)) party.contacts = input.contacts as MockParty['contacts'];
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
