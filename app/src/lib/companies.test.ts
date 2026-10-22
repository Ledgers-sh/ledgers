import { describe, expect, it } from 'vitest';
import {
  companyFromSearch,
  companyHref,
  companyNameProblem,
  currencyProblem,
  fiscalYearStartProblem,
  roleBadgeClass,
  searchWithCompany,
} from './companies';

describe('the company in the URL', () => {
  it('reads a company id from the query and ignores anything malformed', () => {
    expect(companyFromSearch('?company=01HALDEN')).toBe('01HALDEN');
    expect(companyFromSearch('?unit=LDG&company=01HALDEN')).toBe('01HALDEN');
    expect(companyFromSearch('')).toBeNull();
    expect(companyFromSearch('?company=')).toBeNull();
    expect(companyFromSearch('?company=a%20b')).toBeNull();
    expect(companyFromSearch(`?company=${'x'.repeat(65)}`)).toBeNull();
  });

  it('round-trips: the query written for a company reads back as that company', () => {
    const search = searchWithCompany('', '01HALDEN');
    expect(search).toBe('?company=01HALDEN');
    expect(companyFromSearch(search)).toBe('01HALDEN');
  });

  it('keeps the other parameters when it sets or removes the company', () => {
    expect(searchWithCompany('?period=2026-10&company=OLD', 'NEW')).toBe('?period=2026-10&company=NEW');
    expect(searchWithCompany('?period=2026-10&company=OLD', null)).toBe('?period=2026-10');
    expect(searchWithCompany('?company=OLD', null)).toBe('');
  });

  it('switches to a company on the overview with no unit scope', () => {
    expect(companyHref('/', '01HALDEN')).toBe('/?company=01HALDEN#/overview');
    expect(companyHref('/app/', 'ZZ99')).toBe('/app/?company=ZZ99#/overview');
  });
});

describe('the create dialog checks', () => {
  it('needs a name of 1 to 200 characters once trimmed', () => {
    expect(companyNameProblem('   ')).toBe('empty');
    expect(companyNameProblem(' Acme GmbH ')).toBeNull();
    expect(companyNameProblem('a'.repeat(200))).toBeNull();
    expect(companyNameProblem('a'.repeat(201))).toBe('too-long');
  });

  it('takes an uppercase three-letter currency and a month from 1 to 12', () => {
    expect(currencyProblem('EUR')).toBe(false);
    expect(currencyProblem('eur')).toBe(true);
    expect(currencyProblem('EU')).toBe(true);
    expect(fiscalYearStartProblem(1)).toBe(false);
    expect(fiscalYearStartProblem(12)).toBe(false);
    expect(fiscalYearStartProblem(0)).toBe(true);
    expect(fiscalYearStartProblem(13)).toBe(true);
  });

  it('gives each known role its badge and an unknown one the plain badge', () => {
    expect(roleBadgeClass('owner')).toBe('role-badge role-owner');
    expect(roleBadgeClass('auditor')).toBe('role-badge role-auditor');
    expect(roleBadgeClass('wizard')).toBe('role-badge');
  });
});
