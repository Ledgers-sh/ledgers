/**
 * Screenshots of the business-unit screens for review (not a pass/fail check):
 * the switcher and the by-unit comparison, the units screen, the recharge
 * dialog, a new entry with units, in dark and light, desktop and phone, and
 * Arabic (right-to-left). Written to LEDGERS_SHOTS_DIR, or app/screenshots-units.
 */
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test, type Page } from '@playwright/test';
import { mockApi, signIn, signInPerson, stopMockApi } from './mockApi';

const here = path.dirname(fileURLToPath(import.meta.url));
const outDir = process.env.LEDGERS_SHOTS_DIR ?? path.resolve(here, '../../screenshots-units');

test.afterAll(stopMockApi);

const viewports = [
  { label: 'desktop', width: 1440, height: 900 },
  { label: 'phone', width: 390, height: 844 },
] as const;

const looks = [
  { locale: 'en', theme: 'dark' },
  { locale: 'en', theme: 'light' },
  { locale: 'ar', theme: 'dark' },
] as const;

async function settle(page: Page) {
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(250);
}

for (const viewport of viewports) {
  for (const look of looks) {
    const name = `${viewport.label}-${look.locale}-${look.theme}`;
    test(`screens ${name}`, async ({ page }) => {
      await page.setViewportSize({ width: viewport.width, height: viewport.height });
      await mockApi({ seed: true, units: true, person: true });
      await signInPerson(page, look.theme, look.locale);
      await page.goto('/#/overview');
      await expect(page.locator('.units-summary')).toBeVisible();
      await settle(page);
      await page.screenshot({ path: path.join(outDir, `overview-${name}.png`), fullPage: true });

      await page.goto('/#/overview?unit=LDG');
      await expect(page.getByRole('combobox', { name: /Business unit|الوحدة التجارية/ }).first()).toBeVisible();
      await settle(page);
      await page.screenshot({ path: path.join(outDir, `overview-unit-${name}.png`), fullPage: true });

      await page.goto('/#/accounts/units');
      await expect(page.getByRole('table')).toBeVisible();
      await settle(page);
      await page.screenshot({ path: path.join(outDir, `units-${name}.png`), fullPage: true });

      await page.getByRole('button', { name: /تحويل بين الوحدات|Transfer between units/ }).first().click();
      await expect(page.locator('dialog[open]')).toBeVisible();
      await settle(page);
      await page.screenshot({ path: path.join(outDir, `transfer-${name}.png`) });
      await page.keyboard.press('Escape');

      await signIn(page, look.theme, look.locale);
      await page.goto('/#/new?unit=FZ');
      await expect(page.locator('.line-row')).toHaveCount(2);
      await settle(page);
      await page.screenshot({ path: path.join(outDir, `new-entry-${name}.png`) });
    });
  }
}

test('the sidebar fills the window on a tall page, as a full-page capture shows', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await mockApi({ seed: true, extraEntries: 120 });
  await signIn(page, 'dark');
  await page.goto('/#/journal');
  await expect(page.locator('.sidebar')).toBeVisible();
  await settle(page);
  await page.evaluate(() => window.scrollTo(0, document.documentElement.scrollHeight));
  await page.waitForTimeout(150);
  await page.screenshot({ path: path.join(outDir, 'sidebar-journal-scrolled-end.png') });
});
