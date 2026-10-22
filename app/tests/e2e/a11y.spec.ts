/**
 * axe-core on every main screen, in English, German and Arabic (RTL), with
 * the command palette and the language picker open too. Fails on any serious
 * or critical WCAG 2.2 AA violation.
 */
import AxeBuilder from '@axe-core/playwright';
import { expect, test, type Page } from '@playwright/test';
import { mockApi, signIn, stopMockApi } from './mockApi';

const LOCALES = ['en', 'de', 'ar'] as const;

const SCREENS = [
  { name: 'overview', hash: '#/overview' },
  { name: 'journal', hash: '#/journal' },
  { name: 'entry', hash: '#/journal/seed-2' },
  { name: 'new-entry', hash: '#/new' },
  { name: 'accounts', hash: '#/accounts' },
  { name: 'reports', hash: '#/reports' },
];

async function blockingViolations(page: Page) {
  const results = await new AxeBuilder({ page })
    .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa'])
    .analyze();
  const blocking = results.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical');
  return blocking.map((v) => `${v.impact} ${v.id}: ${v.help} (${v.nodes.length} nodes)`);
}

test.afterAll(stopMockApi);

for (const locale of LOCALES) {
  test.describe(`locale ${locale}`, () => {
    test.beforeEach(async () => {
      await mockApi({ seed: true });
    });

    test('sign-in screen', async ({ page }) => {
      await page.addInitScript((code) => window.localStorage.setItem('ledgers.locale', code), locale);
      await page.goto('/');
      await expect(page.locator('main.connect h1')).toBeVisible();
      expect(await blockingViolations(page)).toEqual([]);
    });

    for (const screen of SCREENS) {
      test(`${screen.name} screen`, async ({ page }) => {
        await signIn(page, 'dark', locale);
        await page.goto(`/${screen.hash}`);
        await expect(page.locator('h1').first()).toBeVisible();
        await page.waitForLoadState('networkidle');
        expect(await blockingViolations(page)).toEqual([]);
      });
    }

    test('command palette open', async ({ page }) => {
      await signIn(page, 'dark', locale);
      await page.goto('/#/overview');
      await expect(page.locator('h1').first()).toBeVisible();
      await page.keyboard.press('Control+k');
      await expect(page.locator('dialog.palette')).toBeVisible();
      await expect(page.locator('.palette-item').first()).toBeVisible();
      expect(await blockingViolations(page)).toEqual([]);
    });

    test('language picker open', async ({ page }) => {
      await signIn(page, 'dark', locale);
      await page.goto('/#/overview');
      await expect(page.locator('h1').first()).toBeVisible();
      await page.locator('.lang-button').click();
      await expect(page.getByRole('combobox')).toBeFocused();
      expect(await blockingViolations(page)).toEqual([]);
    });
  });
}
