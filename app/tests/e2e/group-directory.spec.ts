/**
 * The Group screens for a company that is in no group. The person owns three
 * companies and one group (Factory Zero IP and PT Operasional Nusantara). The
 * Group screen on Halden Works, which is in no group, lists the group with its
 * logo and member companies and opens it in a member company's group view.
 * The group view shows each company's business units with their logos.
 */
import { expect, test, type Page } from '@playwright/test';
import { mockApi, signInPerson, stopMockApi } from './mockApi';
import { companyState } from './mockCompanies';
import { groupsMock } from './mockGroups';

test.afterAll(stopMockApi);

/** Three companies the person owns; Halden Works is in no group. */
function ownedCompanies(): void {
  companyState.companies = [
    { id: 'HALDEN01', name: 'Halden Works GmbH', baseCurrency: 'EUR', role: 'owner' },
    { id: 'FZIP01', name: 'Factory Zero IP Pte. Ltd.', baseCurrency: 'EUR', role: 'owner', logo_document_id: 'DOCFZIP' },
    { id: 'PTOP01', name: 'PT Operasional Nusantara', baseCurrency: 'EUR', role: 'owner' },
  ] as typeof companyState.companies;
}

async function imageLoaded(page: Page, selector: string): Promise<boolean> {
  return page.locator(selector).first().evaluate((el) => (el as HTMLImageElement).complete && (el as HTMLImageElement).naturalWidth > 0);
}

test.describe('group: a company in no group', () => {
  test('lists the groups the person sees, with logos and member companies, and opens one', async ({ page }) => {
    await mockApi({ seed: true, person: true, groups: true });
    ownedCompanies();
    groupsMock.groups = [
      {
        id: 'FZGROUP01',
        name: 'Factory Zero Group',
        logo_document_id: 'DOCFZ',
        logo_company_id: 'FZIP01',
        members: [
          { company_id: 'FZIP01', kind: 'ip_holding', ownership_bp: 10000 },
          { company_id: 'PTOP01', kind: 'operating', ownership_bp: 10000 },
        ],
      },
    ];
    await signInPerson(page);
    await page.goto('/?company=HALDEN01#/group');

    // The Group nav is reachable from a company in no group, because a group is visible through another company.
    await expect(page.getByRole('link', { name: 'Group', exact: true })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'Factory Zero Group', level: 2 })).toBeVisible();
    await expect(page.getByText(/not in a group/)).toBeVisible();
    // PT Operasional has no logo: its avatar shows its initials on a colour.
    await expect(page.locator('[data-testid="avatar-initials"]', { hasText: 'PO' }).first()).toBeVisible();
    const members = page.getByRole('list', { name: 'Member companies' });
    await expect(members).toContainText('Factory Zero IP Pte. Ltd.');
    await expect(members).toContainText('PT Operasional Nusantara');
    await expect(page.getByRole('button', { name: 'Open group Factory Zero Group' })).toBeVisible();
    // A member with a logo shows it (the signed link serves the PNG).
    await expect.poll(() => imageLoaded(page, 'img.avatar')).toBe(true);

    await page.getByRole('button', { name: 'Open group Factory Zero Group' }).click();
    await expect(page).toHaveURL(/company=FZIP01.*#\/group\/view/);
    await expect(page.getByText('Combined consolidation is a Ledgers Cloud feature')).toBeVisible();
  });

  test('with two groups, the header selector shows one of them', async ({ page }) => {
    await mockApi({ seed: true, person: true, groups: true });
    ownedCompanies();
    groupsMock.groups = [
      { id: 'FZGROUP01', name: 'Factory Zero Group', members: [{ company_id: 'FZIP01', kind: 'ip_holding', ownership_bp: 10000 }, { company_id: 'PTOP01', kind: 'operating', ownership_bp: 10000 }] },
      { id: 'SECOND01', name: 'Second Group', members: [{ company_id: 'PTOP01', kind: 'operating', ownership_bp: 10000 }, { company_id: 'FZIP01', kind: 'holding', ownership_bp: 10000 }] },
    ];
    await signInPerson(page);
    await page.goto('/?company=HALDEN01#/group');
    await expect(page.getByRole('heading', { name: 'Factory Zero Group', level: 2 })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'Second Group', level: 2 })).toBeVisible();
    await page.locator('#directory-group').click();
    await page.getByRole('option', { name: 'Second Group' }).click();
    await expect(page.getByRole('heading', { name: 'Second Group', level: 2 })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'Factory Zero Group', level: 2 })).toHaveCount(0);
  });

  test('the group view lists each business unit with its logo and month figures', async ({ page }) => {
    await mockApi({ seed: true, person: true, groups: true });
    ownedCompanies();
    groupsMock.unitsEnabled = true;
    groupsMock.groups = [
      { id: 'FZGROUP01', name: 'Factory Zero Group', members: [{ company_id: 'FZIP01', kind: 'ip_holding', ownership_bp: 10000 }, { company_id: 'PTOP01', kind: 'operating', ownership_bp: 10000 }] },
    ];
    await signInPerson(page);
    await page.goto('/?company=FZIP01#/group/view');
    const ledgers = page.locator('.unit-figure').filter({ hasText: 'Ledgers' });
    await expect(ledgers).toBeVisible();
    await expect(ledgers.locator('img.avatar')).toHaveCount(1);
    await expect(ledgers).toContainText('EUR');
    await expect(page.locator('.unit-figure').filter({ hasText: 'Sqeezin' })).toBeVisible();
    await expect(page.locator('.unit-figure').filter({ hasText: 'Factory Zero' }).first()).toBeVisible();
  });
});
