import { describe, expect, it } from 'vitest';
import type { Party, PartyDetail } from '../api/parties';
import {
  bankChangeLines,
  canManageBank,
  compactIban,
  compactVat,
  countryName,
  createInputFromDraft,
  draftFromParty,
  emptyDraft,
  fieldErrors,
  formatIban,
  listQuery,
  mergePreview,
  reviewQuery,
  rolesQuery,
  termsValue,
  updateInputFromDraft,
  validateInputFromDraft,
  vatCountry,
  vatStatus,
} from './parties';

function party(over: Partial<Party> = {}): Party {
  return {
    id: 'p1',
    name: 'Halden Works GmbH',
    vat_id: 'DE136695976',
    iban: 'DE89370400440532013000',
    bic: null,
    roles: 'vendor',
    merged_into: null,
    latest_vies: null,
    personal_data: false,
    possible_duplicates: [],
    needs_review: false,
    tax_number: null,
    language: null,
    currency: 'EUR',
    payment_terms_days: 30,
    default_account: null,
    default_tax_code: null,
    addresses: [{ kind: 'billing', line1: 'Musterstrasse 1', line2: null, postal_code: '10115', city: 'Berlin', country: 'DE' }],
    contacts: [{ name: 'Lea', email: 'lea@example.test', phone: null, role: null }],
    ...over,
  };
}

function detail(p: Party, bills = 0): PartyDetail {
  return { party: p, bank_changes: [], bill_count: bills, vies_enabled: true };
}

describe('VAT status and IDs', () => {
  it('reads the chip from the VAT ID and the latest VIES answer', () => {
    expect(vatStatus({ vat_id: null, latest_vies: null })).toBe('none');
    expect(vatStatus({ vat_id: 'DE1', latest_vies: null })).toBe('unchecked');
    expect(vatStatus({ vat_id: 'DE1', latest_vies: { valid: true, consultation_number: null, checked_at: '' } })).toBe('valid');
    expect(vatStatus({ vat_id: 'DE1', latest_vies: { valid: false, consultation_number: null, checked_at: '' } })).toBe('invalid');
  });

  it('compacts a VAT ID the way the server normalises it, and reads its country', () => {
    expect(compactVat(' de 136.695-976 ')).toBe('DE136695976');
    expect(vatCountry('de136695976')).toBe('DE');
    expect(vatCountry('136695976')).toBeNull();
  });
});

describe('IBAN', () => {
  it('compacts and groups an IBAN', () => {
    expect(compactIban(' de89 3704 0044 0532 0130 00 ')).toBe('DE89370400440532013000');
    expect(formatIban('DE89370400440532013000')).toBe('DE89 3704 0044 0532 0130 00');
  });

  it('describes a bank change as before and after', () => {
    expect(
      bankChangeLines({ before_iban: null, after_iban: 'GB82WEST12345698765432', before_bic: null, after_bic: 'WESTGB2L' }),
    ).toEqual({ before: null, after: 'GB82 WEST 1234 5698 7654 32', bicBefore: null, bicAfter: 'WESTGB2L' });
  });
});

describe('list requests', () => {
  it('sends only the filters that are set, one page at a time', () => {
    expect(rolesQuery('all')).toBeUndefined();
    expect(rolesQuery('both')).toBe('both');
    expect(reviewQuery('needs')).toBe(true);
    expect(reviewQuery('reviewed')).toBe(false);
    expect(reviewQuery('all')).toBeUndefined();
    expect(listQuery({ q: '  acme ', roles: 'customer', review: 'all' }, 2)).toEqual({
      limit: 50,
      offset: 100,
      q: 'acme',
      roles: 'customer',
      needs_review: undefined,
    });
    expect(listQuery({ q: '   ', roles: 'all', review: 'needs' }, 0).q).toBeUndefined();
  });
});

describe('permissions on screen', () => {
  it('lets only owners and admins change bank details or merge', () => {
    expect(canManageBank('owner')).toBe(true);
    expect(canManageBank('admin')).toBe(true);
    expect(canManageBank('accountant')).toBe(false);
    expect(canManageBank('viewer')).toBe(false);
    expect(canManageBank(null)).toBe(false);
  });
});

describe('form drafts and the payloads they send', () => {
  it('reads payment terms as whole days, blank as none, and anything else as refused', () => {
    expect(termsValue('')).toBeNull();
    expect(termsValue(' 30 ')).toBe(30);
    expect(termsValue('0')).toBe(0);
    expect(Number.isNaN(termsValue('thirty'))).toBe(true);
    expect(Number.isNaN(termsValue('1.5'))).toBe(true);
  });

  it('sends blanks as none on create and as empty text on update, so a cleared field clears', () => {
    const draft = { ...draftFromParty(party({ tax_number: 'DE-1' })), tax_number: '  ', default_account: '', currency: 'eur' };
    const created = createInputFromDraft(draft);
    expect(created.tax_number).toBeNull();
    expect(created.currency).toBe('EUR');
    const updated = updateInputFromDraft(draft);
    expect(updated.tax_number).toBe('');
    expect(updated.default_account).toBe('');
    expect(updated).not.toHaveProperty('iban');
    expect(updated).not.toHaveProperty('bic');
  });

  it('round-trips a party into the form and back into the same addresses and contacts', () => {
    const p = party();
    const draft = draftFromParty(p);
    expect(draft.addresses[0].country).toBe('DE');
    expect(updateInputFromDraft(draft).addresses).toEqual(p.addresses);
    expect(updateInputFromDraft(draft).contacts).toEqual(p.contacts);
  });

  it('validates only the fields the form holds, and an unset terms field is not checked', () => {
    const draft = emptyDraft('both', '  ');
    const v = validateInputFromDraft(draft, 'p9');
    expect(v).toMatchObject({ id: 'p9', name: '  ', roles: 'both' });
    expect(v).not.toHaveProperty('payment_terms_days');
    expect(validateInputFromDraft({ ...draft, payment_terms_days: '12' }).payment_terms_days).toBe(12);
  });

  it('keeps the first server message for each field', () => {
    expect(
      fieldErrors([
        { field: 'vat_id', message: 'first' },
        { field: 'vat_id', message: 'second' },
        { field: 'name', message: 'needs a name' },
      ]),
    ).toEqual({ vat_id: 'first', name: 'needs a name' });
  });
});

describe('merge preview', () => {
  it('says what moves and what stays, from the two parties on screen', () => {
    const source = detail(party({ id: 'src', iban: null, vat_id: null, addresses: [] }), 3);
    const target = party({ id: 'dst' });
    expect(mergePreview(source, target)).toEqual({
      ok: true,
      reason: null,
      billsMove: 3,
      keepsVat: false,
      keepsBank: false,
      keepsAddresses: 0,
      keepsContacts: 1,
    });
  });

  it('refuses a merge into itself, into a merged party, or from a merged party, and says why', () => {
    const source = detail(party({ id: 'a' }));
    expect(mergePreview(source, party({ id: 'a' })).reason).toBe('same');
    expect(mergePreview(source, party({ id: 'b', merged_into: 'c' })).reason).toBe('survivor-merged');
    expect(mergePreview(detail(party({ id: 'a', merged_into: 'b' })), party({ id: 'b' })).reason).toBe('merged');
    expect(mergePreview(source, null).ok).toBe(false);
  });
});

describe('country names', () => {
  it('names a country in the reader language, and falls back to the code', () => {
    expect(countryName('DE', 'de')).toBe('Deutschland');
    expect(countryName('DE', 'en')).toBe('Germany');
    expect(countryName('ZZ', 'en')).toBe('ZZ');
  });
});
