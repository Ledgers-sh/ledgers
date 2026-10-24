/**
 * Customers and vendors: the pure rules the screens share. Nothing here calls
 * the server; the server's validators decide what is valid (see parties.validate).
 */
import { t } from '../i18n';
import { slugMessageKey } from './problems';
import type {
  Party,
  PartyAddress,
  PartyContact,
  PartyCreateInput,
  PartyDetail,
  PartyListQuery,
  PartyRoles,
  PartyTaxId,
  PartyUpdateInput,
  PartyValidateInput,
} from '../api/parties';

export const ROLE_FILTERS = ['all', 'customer', 'vendor', 'both'] as const;
export type RoleFilter = (typeof ROLE_FILTERS)[number];

export const REVIEW_FILTERS = ['all', 'needs', 'reviewed'] as const;
export type ReviewFilter = (typeof REVIEW_FILTERS)[number];

export const PAGE_SIZE = 50;

/** The VAT check shown as a chip: a valid or invalid VIES answer, unchecked, or no VAT ID at all. */
export type VatStatus = 'valid' | 'invalid' | 'unchecked' | 'none';

export function vatStatus(party: Pick<Party, 'vat_id' | 'latest_vies'>): VatStatus {
  if (!party.vat_id) return 'none';
  if (!party.latest_vies) return 'unchecked';
  return party.latest_vies.valid ? 'valid' : 'invalid';
}

/** The language key of a role, as the forms and chips name it. */
export function roleLabelKey(roles: PartyRoles): 'parties.role_customer' | 'parties.role_vendor' | 'parties.role_both' {
  if (roles === 'customer') return 'parties.role_customer';
  if (roles === 'both') return 'parties.role_both';
  return 'parties.role_vendor';
}

/** The role filter as the list request names it: `all` leaves the filter off. */
export function rolesQuery(filter: RoleFilter): PartyRoles | undefined {
  return filter === 'all' ? undefined : filter;
}

/** The review filter as the list request names it: `all` leaves the filter off. */
export function reviewQuery(filter: ReviewFilter): boolean | undefined {
  if (filter === 'needs') return true;
  if (filter === 'reviewed') return false;
  return undefined;
}

/** The list request for one page and the filters on screen. */
export function listQuery(filters: { q: string; roles: RoleFilter; review: ReviewFilter }, page: number): PartyListQuery {
  return {
    limit: PAGE_SIZE,
    offset: Math.max(0, page) * PAGE_SIZE,
    q: filters.q.trim() === '' ? undefined : filters.q.trim(),
    roles: rolesQuery(filters.roles),
    needs_review: reviewQuery(filters.review),
  };
}

/** Whether a role may change bank details or merge parties: owners and admins only (the server agrees). */
export function canManageBank(role: string | null | undefined): boolean {
  return role === 'owner' || role === 'admin';
}

/** IBAN without spaces, upper case: the form the server stores. */
export function compactIban(raw: string): string {
  return raw.replace(/\s+/g, '').toUpperCase();
}

/** An IBAN in groups of four, as people read it. */
export function formatIban(raw: string): string {
  return compactIban(raw).replace(/(.{4})(?=.)/g, '$1 ');
}

/** A VAT ID without the spaces and dashes people type between groups, upper case. */
export function compactVat(raw: string): string {
  return raw.replace(/[\s.-]+/g, '').toUpperCase();
}

/** The two-letter country a VAT ID starts with, when it does. */
export function vatCountry(vat: string): string | null {
  const prefix = compactVat(vat).slice(0, 2);
  return /^[A-Z]{2}$/.test(prefix) ? prefix : null;
}

export interface AddressDraft {
  kind: string;
  line1: string;
  line2: string;
  postal_code: string;
  city: string;
  country: string;
}

export interface ContactDraft {
  name: string;
  email: string;
  phone: string;
  role: string;
  primary: boolean;
}

/** A further tax identifier as the form holds it. */
export interface TaxIdDraft {
  country: string;
  kind: PartyTaxId['kind'];
  value: string;
}

/** The form's state. Numbers and lists are text until they are sent. */
export interface PartyDraft {
  name: string;
  trading_name: string;
  roles: PartyRoles;
  vat_id: string;
  tax_ids: TaxIdDraft[];
  tax_number: string;
  registration_number: string;
  website: string;
  email: string;
  phone: string;
  language: string;
  currency: string;
  payment_terms_days: string;
  default_account: string;
  default_tax_code: string;
  default_unit: string;
  /** Tags as the person typed them: separated by commas or semicolons. */
  tags: string;
  notes: string;
  personal_data: boolean;
  addresses: AddressDraft[];
  contacts: ContactDraft[];
}

export function emptyAddress(): AddressDraft {
  return { kind: 'billing', line1: '', line2: '', postal_code: '', city: '', country: '' };
}

export function emptyContact(): ContactDraft {
  return { name: '', email: '', phone: '', role: '', primary: false };
}

export function emptyTaxId(): TaxIdDraft {
  return { country: '', kind: 'vat', value: '' };
}

export function emptyDraft(roles: PartyRoles = 'vendor', name = ''): PartyDraft {
  return {
    name,
    trading_name: '',
    roles,
    vat_id: '',
    tax_ids: [],
    tax_number: '',
    registration_number: '',
    website: '',
    email: '',
    phone: '',
    language: '',
    currency: '',
    payment_terms_days: '',
    default_account: '',
    default_tax_code: '',
    default_unit: '',
    tags: '',
    notes: '',
    personal_data: false,
    addresses: [],
    contacts: [],
  };
}

/** Tags as the server keeps them: trimmed, lower-cased, without repeats, split on commas or semicolons. */
export function splitTags(text: string): string[] {
  const out: string[] = [];
  for (const raw of text.split(/[,;]/)) {
    const tag = raw.trim().toLowerCase();
    if (tag !== '' && !out.includes(tag)) out.push(tag);
  }
  return out;
}

export function draftFromParty(party: Party): PartyDraft {
  return {
    name: party.name,
    trading_name: party.trading_name ?? '',
    roles: party.roles,
    vat_id: party.vat_id ?? '',
    tax_ids: party.tax_ids.map((t) => ({ country: t.country, kind: t.kind, value: t.value })),
    tax_number: party.tax_number ?? '',
    registration_number: party.registration_number ?? '',
    website: party.website ?? '',
    email: party.email ?? '',
    phone: party.phone ?? '',
    language: party.language ?? '',
    currency: party.currency ?? '',
    payment_terms_days: party.payment_terms_days === null ? '' : String(party.payment_terms_days),
    default_account: party.default_account ?? '',
    default_tax_code: party.default_tax_code ?? '',
    default_unit: party.default_unit ?? '',
    tags: party.tags.join(', '),
    notes: party.notes ?? '',
    personal_data: party.personal_data,
    addresses: party.addresses.map((a) => ({
      kind: a.kind ?? '',
      line1: a.line1,
      line2: a.line2 ?? '',
      postal_code: a.postal_code ?? '',
      city: a.city,
      country: a.country,
    })),
    contacts: party.contacts.map((c) => ({ name: c.name, email: c.email ?? '', phone: c.phone ?? '', role: c.role ?? '', primary: c.primary })),
  };
}

/** Whole days, or null when the text is blank. Anything else is left as NaN for the server's check to refuse. */
export function termsValue(text: string): number | null {
  const trimmed = text.trim();
  if (trimmed === '') return null;
  return /^-?\d+$/.test(trimmed) ? Number.parseInt(trimmed, 10) : Number.NaN;
}

function addressOut(a: AddressDraft): PartyAddress {
  return {
    kind: a.kind.trim() || null,
    line1: a.line1.trim(),
    line2: a.line2.trim() || null,
    postal_code: a.postal_code.trim() || null,
    city: a.city.trim(),
    country: a.country.trim().toUpperCase(),
  };
}

function contactOut(c: ContactDraft): PartyContact {
  return {
    name: c.name.trim(),
    email: c.email.trim() || null,
    phone: c.phone.trim() || null,
    role: c.role.trim() || null,
    primary: c.primary,
  };
}

/** The further tax IDs that have any text, as the server names them. */
function taxIdsOut(list: TaxIdDraft[]): PartyTaxId[] {
  return list
    .filter((t) => t.country.trim() !== '' || t.value.trim() !== '')
    .map((t) => ({ country: t.country.trim().toUpperCase(), kind: t.kind, value: t.value.trim() }));
}

/** The full party a create sends. Blank optional text is sent as none. */
export function createInputFromDraft(draft: PartyDraft): PartyCreateInput {
  return {
    name: draft.name.trim(),
    trading_name: draft.trading_name.trim() || null,
    roles: draft.roles,
    vat_id: compactVat(draft.vat_id) || null,
    tax_ids: taxIdsOut(draft.tax_ids),
    tax_number: draft.tax_number.trim() || null,
    registration_number: draft.registration_number.trim() || null,
    website: draft.website.trim() || null,
    email: draft.email.trim() || null,
    phone: draft.phone.trim() || null,
    language: draft.language.trim() || null,
    currency: draft.currency.trim().toUpperCase() || null,
    personal_data: draft.personal_data,
    payment_terms_days: termsValue(draft.payment_terms_days),
    default_account: draft.default_account.trim() || null,
    default_tax_code: draft.default_tax_code.trim() || null,
    default_unit: draft.default_unit.trim() || null,
    tags: splitTags(draft.tags),
    notes: draft.notes.trim() || null,
    addresses: draft.addresses.map(addressOut),
    contacts: draft.contacts.map(contactOut),
  };
}

/**
 * The master-data fields an update sends. Blank text is sent as an empty string,
 * which the server stores as none, so a cleared field really clears. Bank details
 * are never part of it.
 */
export function updateInputFromDraft(draft: PartyDraft): PartyUpdateInput {
  const created = createInputFromDraft(draft);
  return {
    name: draft.name.trim(),
    trading_name: draft.trading_name.trim(),
    roles: draft.roles,
    vat_id: compactVat(draft.vat_id),
    tax_ids: created.tax_ids,
    tax_number: draft.tax_number.trim(),
    registration_number: draft.registration_number.trim(),
    website: draft.website.trim(),
    email: draft.email.trim(),
    phone: draft.phone.trim(),
    language: draft.language.trim(),
    currency: draft.currency.trim().toUpperCase(),
    personal_data: draft.personal_data,
    payment_terms_days: created.payment_terms_days,
    default_account: draft.default_account.trim(),
    default_tax_code: draft.default_tax_code.trim(),
    default_unit: draft.default_unit.trim(),
    tags: created.tags,
    notes: draft.notes.trim(),
    addresses: created.addresses,
    contacts: created.contacts,
  };
}

/** What `parties.validate` should check for this draft: each field the form holds. */
export function validateInputFromDraft(draft: PartyDraft, id?: string): PartyValidateInput {
  const terms = termsValue(draft.payment_terms_days);
  return {
    id,
    name: draft.name,
    vat_id: draft.vat_id,
    roles: draft.roles,
    ...(terms === null ? {} : { payment_terms_days: terms }),
    addresses: draft.addresses.map(addressOut),
    contacts: draft.contacts.map(contactOut),
    registration_number: draft.registration_number,
    website: draft.website,
    email: draft.email,
    phone: draft.phone,
    currency: draft.currency,
    language: draft.language,
    default_unit: draft.default_unit,
    notes: draft.notes,
    tags: splitTags(draft.tags),
    tax_ids: taxIdsOut(draft.tax_ids),
  };
}

/**
 * A map from a form field to its message: the app's own words for the
 * refusal's problem slug when it has them, the server's message otherwise.
 */
export function fieldErrors(errors: { field: string; message: string; slug?: string | null }[]): Record<string, string> {
  const out: Record<string, string> = {};
  for (const error of errors) {
    if (error.field in out) continue;
    const key = error.slug ? slugMessageKey(error.slug) : null;
    out[error.field] = key ? t(key) : error.message;
  }
  return out;
}

export interface MergePreview {
  /** False when the two parties cannot be merged, with the reason key. */
  ok: boolean;
  reason: 'same' | 'merged' | 'survivor-merged' | null;
  /** Bills that repoint from the merged party to the survivor. */
  billsMove: number;
  /** Whether the merged party's own VAT ID stays on its (merged) record. */
  keepsVat: boolean;
  /** Whether the merged party's bank details stay on its (merged) record. */
  keepsBank: boolean;
  /** Addresses and contacts that stay on the merged record, not moved. */
  keepsAddresses: number;
  keepsContacts: number;
}

/** What a merge of `source` into `survivor` will do, read from the two parties on screen. */
export function mergePreview(source: PartyDetail, survivor: Party | null): MergePreview {
  const none = { billsMove: source.bill_count, keepsVat: !!source.party.vat_id, keepsBank: !!source.party.iban, keepsAddresses: source.party.addresses.length, keepsContacts: source.party.contacts.length };
  if (!survivor) return { ok: false, reason: null, ...none };
  if (survivor.id === source.party.id) return { ok: false, reason: 'same', ...none };
  if (source.party.merged_into) return { ok: false, reason: 'merged', ...none };
  if (survivor.merged_into) return { ok: false, reason: 'survivor-merged', ...none };
  return { ok: true, reason: null, ...none };
}

/** Open payables of a party in the currencies it has, as minor-unit amounts. */
export function payablesOf(balances: { party_id: string; currency: string; payable_minor: number; open_bills: number }[], partyId: string) {
  return balances.filter((b) => b.party_id === partyId);
}

/** The bank-details change as a before and after pair for the history list. */
export function bankChangeLines(change: { before_iban: string | null; after_iban: string | null; before_bic: string | null; after_bic: string | null }) {
  return {
    before: change.before_iban ? formatIban(change.before_iban) : null,
    after: change.after_iban ? formatIban(change.after_iban) : null,
    bicBefore: change.before_bic,
    bicAfter: change.after_bic,
  };
}

/** The countries offered for addresses: the EU members plus the common trading partners. */
export const COUNTRY_CODES = [
  'DE', 'AT', 'BE', 'BG', 'CH', 'CY', 'CZ', 'DK', 'EE', 'ES', 'FI', 'FR', 'GB', 'GR', 'HR', 'HU', 'IE', 'IS', 'IT', 'LI', 'LT', 'LU', 'LV', 'MT', 'NL', 'NO', 'PL', 'PT', 'RO', 'SE', 'SI', 'SK', 'US', 'SG', 'ID', 'AE', 'SA', 'CN', 'JP', 'IN', 'AU', 'CA',
] as const;

/** A country name in the reader's language, from the platform's own names. */
export function countryName(code: string, locale: string): string {
  // Only the codes the form offers get a name; any other code is shown as it is.
  if (!(COUNTRY_CODES as readonly string[]).includes(code)) return code;
  try {
    return new Intl.DisplayNames([locale], { type: 'region' }).of(code) ?? code;
  } catch {
    return code;
  }
}
