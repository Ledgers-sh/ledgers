/**
 * Pictures of the chart of accounts for review (not a pass/fail check beyond
 * rendering): the list, the new-account form, an account page and the archive
 * dialog, at 1440 and 390 wide, dark and light in English, and Arabic (RTL)
 * in dark. Written to ACCOUNTS_SHOTS_DIR, or app/screenshots/accounts.
 */
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test, type Page } from '@playwright/test';
import { mockApi, signInPerson, stopMockApi } from './mockApi';

const here = path.dirname(fileURLToPath(import.meta.url));
const outDir = process.env.ACCOUNTS_SHOTS_DIR ?? path.resolve(here, '../../screenshots/accounts');

test.afterAll(stopMockApi);

const combinations = [
  { locale: 'en', theme: 'dark' },
  { locale: 'en', theme: 'light' },
  { locale: 'ar', theme: 'dark' },
] as const;

const viewports = [
  { label: 'desktop', width: 1440, height: 900 },
  { label: 'phone', width: 390, height: 844 },
] as const;

async function shoot(page: Page, file: string) {
  await page.waitForLoadState('networkidle');
  await page.addStyleTag({ content: '.bottom-nav { position: static !important; margin-top: 24px; }' });
  await page.screenshot({ path: path.join(outDir, file), fullPage: true });
}

for (const viewport of viewports) {
  for (const { locale, theme } of combinations) {
    test.describe(`${viewport.label} ${locale} ${theme}`, () => {
      test.use({ viewport: { width: viewport.width, height: viewport.height } });
      const prefix = `${viewport.label}-${locale}-${theme}`;

      test.beforeAll(() => fs.mkdirSync(outDir, { recursive: true }));

      for (const [name, hash] of [
        ['list', '#/accounts'],
        ['new', '#/accounts/new'],
        ['account', '#/accounts/1000'],
      ] as const) {
        test(name, async ({ page }) => {
          await mockApi({ seed: true, person: true });
          await signInPerson(page, theme, locale);
          await page.goto(`/${hash}`);
          await expect(page.locator('h1').first()).toBeVisible();
          await shoot(page, `${prefix}-${name}.png`);
        });
      }

      test('archive dialog', async ({ page }) => {
        await mockApi({ seed: true, person: true });
        await signInPerson(page, theme, locale);
        await page.goto('/#/accounts/1000');
        await page.getByRole('button', { name: locale === 'ar' ? 'أرشفة الحساب' : 'Archive account' }).click();
        await expect(page.getByRole('dialog')).toBeVisible();
        await shoot(page, `${prefix}-archive-dialog.png`);
      });
    });
  }
}
