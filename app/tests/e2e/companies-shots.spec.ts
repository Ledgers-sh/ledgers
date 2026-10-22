/**
 * Screenshots of the company switcher and the create dialog: desktop (1440) and
 * phone (390), dark and light, and Arabic (right to left). The images go to
 * LEDGERS_SHOT_DIR (default: test-results/companies). Run with
 * `npx playwright test tests/e2e/companies-shots.spec.ts`.
 */
import { expect, test, type Page } from '@playwright/test';
import { mockApi, signInPerson, stopMockApi } from './mockApi';
import path from 'node:path';

test.afterAll(stopMockApi);

const DIR = process.env.LEDGERS_SHOT_DIR ?? path.join('test-results', 'companies');
const VIEWS = [
  { name: 'desktop', width: 1440, height: 900 },
  { name: 'phone', width: 390, height: 844 },
] as const;

async function shoot(page: Page, file: string) {
  await page.screenshot({ path: path.join(DIR, file), fullPage: false });
}

for (const view of VIEWS) {
  for (const theme of ['dark', 'light'] as const) {
    for (const locale of ['en', 'ar'] as const) {
      test(`switcher and create dialog: ${view.name} ${theme} ${locale}`, async ({ page }) => {
        await page.setViewportSize({ width: view.width, height: view.height });
        await mockApi({ seed: true, person: true, companies: 'many', units: true });
        await signInPerson(page, theme, locale);
        await page.goto('/?company=HALDEN01#/overview');
        await expect(page.locator('.kpis')).toBeVisible();
        await shoot(page, `${view.name}-${theme}-${locale}-overview.png`);
        await page.getByRole('combobox', { name: /Company|الشركة/ }).click();
        await expect(page.getByRole('listbox').first()).toBeVisible();
        await shoot(page, `${view.name}-${theme}-${locale}-switcher.png`);
        await page.keyboard.press('Escape');

        await page.goto('/?company=HALDEN01#/settings/company');
        await page.getByRole('button', { name: /Create company|Unternehmen anlegen|إنشاء شركة/ }).first().click();
        const dialog = page.locator('dialog[open]');
        await expect(dialog).toBeVisible();
        await shoot(page, `${view.name}-${theme}-${locale}-create.png`);
      });
    }
  }
}
