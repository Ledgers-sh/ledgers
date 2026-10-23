/**
 * Screenshots of the group directory and the group view with logos, at desktop
 * (1440 wide) and phone (390 wide), dark and light. Written to
 * LG_SHOTS_DIR (default: test-results/shots) for a human to look at; the test
 * also fails if a page scrolls sideways at phone width.
 */
import { expect, test } from '@playwright/test';
import { mockApi, signInPerson, stopMockApi } from './mockApi';
import { companyState } from './mockCompanies';
import { groupsMock } from './mockGroups';

test.afterAll(stopMockApi);

const DIR = process.env.LG_SHOTS_DIR ?? 'test-results/shots';
const SIZES = [
  { name: 'desktop', width: 1440, height: 900 },
  { name: 'phone', width: 390, height: 844 },
] as const;

for (const size of SIZES) {
  for (const theme of ['dark', 'light'] as const) {
    test(`group logos ${size.name} ${theme}`, async ({ page }) => {
      await mockApi({ seed: true, person: true, groups: true });
      companyState.companies = [
        { id: 'HALDEN01', name: 'Halden Works GmbH', baseCurrency: 'EUR', role: 'owner' },
        { id: 'FZIP01', name: 'Factory Zero IP Pte. Ltd.', baseCurrency: 'EUR', role: 'owner', logo_document_id: 'DOCFZIP' },
        { id: 'PTOP01', name: 'PT Operasional Nusantara', baseCurrency: 'EUR', role: 'owner' },
      ] as typeof companyState.companies;
      groupsMock.unitsEnabled = true;
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
      await page.setViewportSize({ width: size.width, height: size.height });
      await signInPerson(page, theme);

      await page.goto('/?company=HALDEN01#/group');
      await expect(page.getByRole('heading', { name: 'Factory Zero Group', level: 2 })).toBeVisible();
      await page.screenshot({ path: `${DIR}/directory-${size.name}-${theme}.png`, fullPage: true });

      await page.goto('/?company=FZIP01#/group/view');
      await expect(page.locator('.unit-figure').first()).toBeVisible();
      await page.screenshot({ path: `${DIR}/group-view-${size.name}-${theme}.png`, fullPage: true });

      const sideways = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
      expect(sideways).toBeLessThanOrEqual(0);
    });
  }
}
