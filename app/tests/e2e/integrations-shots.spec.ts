/**
 * Screenshots of Integrations and Costs by venture for review (not a pass/fail
 * check): the gallery, a connection with its mapping and history, the connect
 * form, and the Overview costs chart, in dark and light, desktop and phone, and
 * Arabic (right to left). Written to LEDGERS_SHOTS_DIR, or app/screenshots-integrations.
 */
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test, type Page } from '@playwright/test';
import { mockApi, signInPerson, stopMockApi } from './mockApi';

const here = path.dirname(fileURLToPath(import.meta.url));
const outDir = process.env.LEDGERS_SHOTS_DIR ?? path.resolve(here, '../../screenshots-integrations');

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
  await page.waitForTimeout(300);
}

for (const viewport of viewports) {
  for (const look of looks) {
    const name = `${viewport.label}-${look.locale}-${look.theme}`;
    test(`screens ${name}`, async ({ page }) => {
      await page.setViewportSize({ width: viewport.width, height: viewport.height });
      await mockApi({ seed: true, units: true, connect: true, person: true });
      await signInPerson(page, look.theme, look.locale);

      await page.goto('/#/overview');
      await expect(page.locator('.costs')).toBeVisible();
      await page.locator('.costs').scrollIntoViewIfNeeded();
      await settle(page);
      await page.locator('.costs').screenshot({ path: path.join(outDir, `costs-${name}.png`) });

      await page.goto('/#/settings/integrations');
      await expect(page.locator('.gallery')).toBeVisible();
      await settle(page);
      await page.screenshot({ path: path.join(outDir, `gallery-${name}.png`), fullPage: true });

      await page.locator('.connection-card button[aria-expanded]').first().click();
      await expect(page.locator('.connection-detail')).toBeVisible();
      await settle(page);
      await page.locator('.connection-detail').screenshot({ path: path.join(outDir, `connection-${name}.png`) });

      await page.locator('.gallery-card button').first().click();
      await expect(page.locator('.connect-form')).toBeVisible();
      await settle(page);
      await page.locator('.connect-form').screenshot({ path: path.join(outDir, `connect-form-${name}.png`) });

      // The AWS form: two credential fields and the least-privilege IAM help.
      await page.locator('.gallery-card', { hasText: 'Amazon Web Services' }).locator('button').first().click();
      await expect(page.locator('.connect-form')).toContainText('ce:GetCostAndUsage');
      await settle(page);
      await page.locator('.connect-form').screenshot({ path: path.join(outDir, `connect-form-aws-${name}.png`) });
    });
  }
}
