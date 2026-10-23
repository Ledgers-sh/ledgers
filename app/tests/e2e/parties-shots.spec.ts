/**
 * Screenshots of the customers and vendors screens: desktop (1440 wide) and
 * phone (390 wide), dark and light, and Arabic (right to left). Writes only
 * when PARTIES_SHOTS_DIR is set; `npm run screenshots` style runs use it.
 */
import fs from 'node:fs';
import path from 'node:path';
import { expect, test, type Page } from '@playwright/test';
import { mockApi, signInPerson, stopMockApi } from './mockApi';
import { resetParties } from './mockParties';

test.afterAll(stopMockApi);

const DIR = process.env.PARTIES_SHOTS_DIR;

async function shot(page: Page, name: string, fullPage = true) {
  if (!DIR) return;
  fs.mkdirSync(DIR, { recursive: true });
  await page.waitForLoadState('networkidle');
  await page.screenshot({ path: path.join(DIR, `${name}.png`), fullPage });
}

const VIEWPORTS = [
  { label: 'desktop', width: 1440, height: 900 },
  { label: 'phone', width: 390, height: 844 },
] as const;

for (const vp of VIEWPORTS) {
  for (const theme of ['dark', 'light'] as const) {
    test.describe(`${vp.label} ${theme}`, () => {
      test.use({ viewport: { width: vp.width, height: vp.height } });

      test('the list, the party page, the form and the two dialogs', async ({ page }) => {
        resetParties();
        await mockApi({ person: true });
        await signInPerson(page, theme, 'en');
        await page.goto('/#/parties');
        await expect(page.locator('table.parties-table tbody tr').first()).toBeVisible();
        await shot(page, `parties-list-${vp.label}-${theme}`);

        await page.goto('/#/parties/party-halden');
        await expect(page.getByRole('heading', { name: 'Halden Works GmbH' })).toBeVisible();
        await shot(page, `parties-page-${vp.label}-${theme}`);

        await page.getByRole('button', { name: 'Change bank details' }).click();
        await expect(page.getByRole('dialog')).toBeVisible();
        await shot(page, `parties-bank-dialog-${vp.label}-${theme}`, false);
        await page.keyboard.press('Escape');

        await page.goto('/#/parties/party-acme');
        await page.getByRole('button', { name: 'Merge into…' }).click();
        const dialog = page.getByRole('dialog');
        await dialog.getByRole('combobox', { name: 'Merge into…' }).fill('Lumen');
        await dialog.getByRole('option', { name: 'Lumen Logistik AG' }).click();
        await expect(dialog.getByRole('heading', { name: 'What will happen' })).toBeVisible();
        await shot(page, `parties-merge-dialog-${vp.label}-${theme}`, false);
        await page.keyboard.press('Escape');

        await page.goto('/#/parties/new');
        await expect(page.getByRole('heading', { name: 'New party' }).first()).toBeVisible();
        await shot(page, `parties-form-${vp.label}-${theme}`);
      });
    });
  }
}

test.describe('arabic (right to left)', () => {
  test.use({ viewport: { width: 1440, height: 900 } });
  test('the list and the party page in Arabic, dark', async ({ page }) => {
    resetParties();
    await mockApi({ person: true });
    await signInPerson(page, 'dark', 'ar');
    await page.goto('/#/parties');
    await expect(page.locator('table.parties-table tbody tr').first()).toBeVisible();
    await expect(page.locator('html')).toHaveAttribute('dir', 'rtl');
    await shot(page, 'parties-list-desktop-dark-ar');
    await page.goto('/#/parties/party-halden');
    await expect(page.getByRole('heading', { name: 'Halden Works GmbH' })).toBeVisible();
    await shot(page, 'parties-page-desktop-dark-ar');
  });
});
