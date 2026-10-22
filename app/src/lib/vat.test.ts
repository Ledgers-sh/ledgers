import { describe, expect, it } from 'vitest';
import { amountProblem, countryProblem, currencyProblem, emailProblem, matchesPattern, normalizeVatId, vatProblem } from './vat';

describe('VAT IDs follow their country', () => {
  it.each([
    ['DE', 'DE123456789'],
    ['AT', 'ATU12345678'],
    ['BE', 'BE0123456789'],
    ['FR', 'FR12345678901'],
    ['NL', 'NL123456789B01'],
    ['IT', 'IT12345678901'],
    ['ES', 'ESX1234567Z'],
    ['GB', 'GB123456789'],
    ['GB', 'GBGD123'],
    ['PL', 'PL1234567890'],
    ['SE', 'SE123456789012'],
    ['DK', 'DK12345678'],
    ['IE', 'IE1234567T'],
    ['XX', 'XX1234'],
  ])('%s accepts %s', (country, vat) => {
    expect(vatProblem(country, vat)).toBeNull();
  });

  it('names the problem: no country, wrong prefix, wrong shape', () => {
    expect(vatProblem('', 'DE123456789')).toBe('country_needed');
    expect(vatProblem('DE', 'FR123456789')).toBe('prefix');
    expect(vatProblem('DE', 'DE12345678')).toBe('format');
    expect(vatProblem('NL', 'NL123456789B1')).toBe('format');
    expect(vatProblem('DE', '')).toBeNull();
  });

  it('normalizes spaces, dots and dashes before checking', () => {
    expect(normalizeVatId(' de 123.456-789 ')).toBe('DE123456789');
    expect(vatProblem('DE', ' de 123.456.789 ')).toBeNull();
  });

  it('matches the class letters exactly', () => {
    expect(matchesPattern('DE123456789', 'DEd9')).toBe(true);
    expect(matchesPattern('DE12345678', 'DEd9')).toBe(false);
    expect(matchesPattern('DEA23456789', 'DEd9')).toBe(false);
  });
});

describe('profile fields', () => {
  it('accepts one mailbox and refuses the rest', () => {
    expect(emailProblem('')).toBe(false);
    expect(emailProblem('books@halden.example')).toBe(false);
    for (const bad of ['plain', 'a@b', 'a b@c.de', '@c.de', 'a@@c.de', 'a@-c.de']) {
      expect(emailProblem(bad), bad).toBe(true);
    }
  });

  it('country is two capitals or empty', () => {
    expect(countryProblem('')).toBe(false);
    expect(countryProblem('de')).toBe(false);
    expect(countryProblem('DEU')).toBe(true);
  });

  it('amounts follow the currency decimals', () => {
    expect(amountProblem('100', 2)).toBe(false);
    expect(amountProblem('100.50', 2)).toBe(false);
    expect(amountProblem('100.505', 2)).toBe(true);
    expect(amountProblem('10.00', 0)).toBe(true);
    expect(amountProblem('0', 2)).toBe(true);
    expect(amountProblem('abc', 2)).toBe(true);
  });

  it('currency codes are three capitals', () => {
    expect(currencyProblem('eur')).toBe(false);
    expect(currencyProblem('EURO')).toBe(true);
  });
});
