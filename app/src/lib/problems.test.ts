import { describe, expect, it } from 'vitest';
import { ApiError } from '../api/client';
import { en } from '../i18n/en';
import { partyProblemKey, slugMessageKey } from './problems';

describe('party problem slugs', () => {
  it('words a known party problem with its catalogue key', () => {
    const error = new ApiError('validation', 'refused', 422, 'the VAT ID is wrong', 'party-vat-invalid');
    expect(partyProblemKey(error)).toBe('problem.party-vat-invalid');
    expect(en['problem.party-vat-invalid']).toMatch(/country's check/);
  });

  it('gives no key for a slug the app has no words for, or for an error that is not an API error', () => {
    expect(partyProblemKey(new ApiError('validation', 'x', 422, null, 'some-other-slug'))).toBeNull();
    expect(partyProblemKey(new ApiError('validation', 'x', 422, null, null))).toBeNull();
    expect(partyProblemKey(new Error('network'))).toBeNull();
  });

  it('maps a row-level slug from a CSV report the same way', () => {
    expect(slugMessageKey('party-bank-needs-person')).toBe('problem.party-bank-needs-person');
    expect(slugMessageKey('party-unknown')).toBeNull();
  });

  it('has a message for every party problem the server can name', () => {
    const slugs = [
      'party-name-required', 'party-vat-invalid', 'party-vat-taken', 'party-iban-invalid', 'party-bic-invalid',
      'party-roles-invalid', 'party-payment-terms-invalid', 'party-address-invalid', 'party-contact-invalid',
      'party-contact-primary-duplicate', 'party-email-invalid', 'party-website-invalid', 'party-phone-invalid',
      'party-registration-invalid', 'party-currency-invalid', 'party-language-invalid', 'party-tax-id-invalid',
      'party-tax-id-duplicate', 'party-tags-invalid', 'party-notes-too-long', 'party-unit-invalid',
      'party-bank-needs-person', 'party-bank-reason-required', 'party-not-found', 'party-merged',
      'party-logo-not-stored', 'party-logo-not-image', 'party-logo-too-large', 'party-import-invalid',
      'party-csv-invalid', 'party-import-too-many-rows', 'party-merge-self', 'party-vies-unavailable',
      'party-value-invalid',
    ];
    for (const slug of slugs) expect(slugMessageKey(slug), slug).not.toBeNull();
  });
});
