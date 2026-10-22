/**
 * Writes screenshots of every main screen, plus the sign-in screen and the
 * command palette, into app/screenshots/ for review:
 *   en in dark, de in light, ar (right-to-left) in dark,
 *   each at desktop 1440 and phone 390.
 * Pass/fail is rendering only; the axe checks live in a11y.spec.ts.
 */
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test, type Page } from '@playwright/test';
import { mockApi, signIn, stopMockApi } from './mockApi';

const here = path.dirname(fileURLToPath(import.meta.url));
const outDir = path.resolve(here, '../../screenshots');

// One fake API server for the whole file: each test resets its ledger; it stops once, at the end.
test.afterAll(stopMockApi);

const combinations = [
  { locale: 'en', theme: 'dark' },
  { locale: 'de', theme: 'light' },
  { locale: 'ar', theme: 'dark' },
] as const;

const viewports = [
  { label: 'desktop', width: 1440, height: 900 },
  { label: 'phone', width: 390, height: 844 },
] as const;

const screens = [
  { name: 'overview', hash: '#/overview' },
  { name: 'journal', hash: '#/journal' },
  { name: 'entry', hash: '#/journal/seed-2' },
  { name: 'new-entry', hash: '#/new' },
  { name: 'accounts', hash: '#/accounts' },
  { name: 'reports', hash: '#/reports' },
] as const;

/** A fixed bottom bar would sit mid-page in a full-page capture; lay it out in flow for the picture. */
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

      test('sign-in screen', async ({ page }) => {
        await mockApi({ seed: true });
        await page.addInitScript(
          ([t, l]) => {
            window.localStorage.setItem('ledgers.theme', t);
            window.localStorage.setItem('ledgers.locale', l);
          },
          [theme, locale],
        );
        await page.goto('/');
        await expect(page.locator('main.connect h1')).toBeVisible();
        await shoot(page, `${prefix}-signin.png`);
      });

      for (const screen of screens) {
        test(screen.name, async ({ page }) => {
          await mockApi({ seed: true });
          await signIn(page, theme, locale);
          await page.goto(`/${screen.hash}`);
          await expect(page.locator('h1').first()).toBeVisible();
          await shoot(page, `${prefix}-${screen.name}.png`);
        });
      }

      test('command palette', async ({ page }) => {
        await mockApi({ seed: true });
        await signIn(page, theme, locale);
        await page.goto('/#/journal');
        await expect(page.locator('h1').first()).toBeVisible();
        await page.keyboard.press('Control+k');
        await expect(page.locator('.palette-item').first()).toBeVisible();
        await page.waitForLoadState('networkidle');
        await page.screenshot({ path: path.join(outDir, `${prefix}-palette.png`) });
      });
    });
  }
}

test.describe('no horizontal scroll at 360px', () => {
  test.use({ viewport: { width: 360, height: 740 } });

  for (const locale of ['en', 'ar'] as const) {
    for (const screen of screens) {
      test(`${locale} ${screen.name}`, async ({ page }) => {
        await mockApi({ seed: true });
        await signIn(page, 'dark', locale);
        await page.goto(`/${screen.hash}`);
        await expect(page.locator('h1').first()).toBeVisible();
        await page.waitForLoadState('networkidle');
        const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
        expect(overflow).toBeLessThanOrEqual(0);
      });
    }
  }
});
