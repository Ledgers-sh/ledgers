/**
 * Writes screenshots of every screen (desktop 1440, phone 390; dark and light)
 * into app/screenshots/ for review. Not a pass/fail check beyond rendering.
 */
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test } from '@playwright/test';
import { mockApi, signIn, stopMockApi } from './mockApi';

const here = path.dirname(fileURLToPath(import.meta.url));
const outDir = path.resolve(here, '../../screenshots');

const screens = [
  { name: 'overview', hash: '#/overview', heading: 'Overview' },
  { name: 'journal', hash: '#/journal', heading: 'Journal' },
  { name: 'entry', hash: '#/journal/seed-2', heading: 'Entry FY' },
  { name: 'new-entry', hash: '#/new', heading: 'New entry' },
  { name: 'accounts', hash: '#/accounts', heading: 'Accounts' },
  { name: 'reports', hash: '#/reports', heading: 'Reports' },
  { name: 'bills', hash: '#/bills', heading: 'Bills match' },
];

for (const viewport of [
  { label: 'desktop', width: 1440, height: 900 },
  { label: 'phone', width: 390, height: 844 },
]) {
  for (const theme of ['dark', 'light'] as const) {
    test.describe(`${viewport.label} ${theme}`, () => {
      test.afterAll(stopMockApi);
      test.use({ viewport: { width: viewport.width, height: viewport.height } });

      test('connect screen', async ({ page }) => {
        await mockApi({ seed: true });
        await page.addInitScript((t) => window.localStorage.setItem('ledgers.theme', t), theme);
        await page.goto('/');
        await expect(page.getByRole('button', { name: 'Connect', exact: true })).toBeVisible();
        await page.screenshot({ path: path.join(outDir, `${viewport.label}-${theme}-connect.png`), fullPage: true });
      });

      for (const screen of screens) {
        test(screen.name, async ({ page }) => {
          await mockApi({ seed: true });
          await signIn(page, theme);
          await page.goto(`/${screen.hash}`);
          await expect(page.getByRole('heading', { name: new RegExp(screen.heading) }).first()).toBeVisible();
          await page.waitForLoadState('networkidle');
          // A fixed bottom bar would sit mid-page in a full-page capture; lay it out in flow for the picture.
          await page.addStyleTag({ content: '.bottom-nav { position: static !important; margin-top: 24px; }' });
          await page.screenshot({ path: path.join(outDir, `${viewport.label}-${theme}-${screen.name}.png`), fullPage: true });
        });
      }
    });
  }
}

test.describe('no horizontal scroll at 360px', () => {
  test.use({ viewport: { width: 360, height: 740 } });
  test.afterAll(stopMockApi);

  for (const screen of screens) {
    test(screen.name, async ({ page }) => {
      await mockApi({ seed: true });
      await signIn(page, 'dark');
      await page.goto(`/${screen.hash}`);
      await expect(page.getByRole('heading', { name: new RegExp(screen.heading) }).first()).toBeVisible();
      await page.waitForLoadState('networkidle');
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
      expect(overflow).toBeLessThanOrEqual(0);
    });
  }
});
