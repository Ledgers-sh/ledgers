/**
 * Screenshots of the group screens for review: desktop (1440) and phone (390),
 * dark and light, and Arabic (RTL). Written to GROUP_SHOTS_DIR (default: the
 * shots folder beside the test results); not part of the CI run.
 */
import { mkdirSync } from 'node:fs';
import path from 'node:path';
import { expect, test, type Page } from '@playwright/test';
import { mockApi, signInPerson, stopMockApi } from './mockApi';

test.afterAll(stopMockApi);

const DIR = process.env.GROUP_SHOTS_DIR ?? path.join(process.cwd(), 'test-results', 'group-shots');

const SCREENS = [
  { name: 'group', hash: '#/group', ready: 'Factory Zero Group' },
  { name: 'agreements', hash: '#/group/agreements', ready: 'Royalty' },
  { name: 'agreement', hash: '#/group/agreements/AG-ID-1', ready: 'Periods' },
  { name: 'agreement-form', hash: '#/group/agreements/new', ready: 'Parties' },
  { name: 'reconciliation', hash: '#/group/reconciliation', ready: 'Open month' },
  { name: 'group-view', hash: '#/group/view', ready: 'Combined consolidation' },
] as const;

async function shoot(page: Page, file: string) {
  mkdirSync(DIR, { recursive: true });
  await page.waitForTimeout(150);
  await page.screenshot({ path: path.join(DIR, file), fullPage: true });
}

async function openScreen(page: Page, hash: string, theme: 'dark' | 'light', locale: 'en' | 'ar', ready: string) {
  await signInPerson(page, theme, locale);
  await page.goto(`/?company=FZIP01${hash}`);
  // Arabic screens are checked by their heading: the English labels are not on the page.
  await expect(ready ? page.getByText(ready).first() : page.getByRole('heading', { level: 1 })).toBeVisible();
}

for (const [label, size] of [
  ['desktop', { width: 1440, height: 900 }],
  ['phone', { width: 390, height: 844 }],
] as const) {
  for (const theme of ['dark', 'light'] as const) {
    test.describe(`${label} ${theme}`, () => {
      test.use({ viewport: size });
      for (const screen of SCREENS) {
        test(`${screen.name}`, async ({ page }) => {
          await mockApi({ seed: true, person: true, groups: true });
          await openScreen(page, screen.hash, theme, 'en', screen.ready);
          if (screen.name === 'agreement-form') {
            await page.locator('#agreement-licensee').click();
            await page.getByRole('option', { name: 'PT Operasional Nusantara' }).click();
            await expect(page.getByText('Country notes')).toBeVisible();
          }
          await shoot(page, `${label}-${theme}-${screen.name}.png`);
        });
      }
      test('the post dialog with both sides side by side', async ({ page }) => {
        await mockApi({ seed: true, person: true, groups: true });
        await openScreen(page, '#/group/agreements/AG-ID-1', theme, 'en', 'Periods');
        await page.getByRole('button', { name: 'Run period' }).click();
        await page.getByRole('button', { name: 'Review and post' }).first().click();
        await expect(page.getByRole('dialog')).toBeVisible();
        await shoot(page, `${label}-${theme}-post-dialog.png`);
      });
    });
  }
}

test.describe('arabic (rtl)', () => {
  test.use({ viewport: { width: 1440, height: 900 } });
  for (const screen of [SCREENS[0], SCREENS[2], SCREENS[4]]) {
    test(`${screen.name}`, async ({ page }) => {
      await mockApi({ seed: true, person: true, groups: true });
      await openScreen(page, screen.hash, 'light', 'ar', '');
      await shoot(page, `ar-light-${screen.name}.png`);
    });
  }
  test('the post dialog', async ({ page }) => {
    await mockApi({ seed: true, person: true, groups: true });
    await openScreen(page, '#/group/agreements/AG-ID-1', 'dark', 'ar', '');
    await page.getByRole('button', { name: 'تشغيل الفترة' }).click();
    await page.getByRole('button', { name: 'مراجعة وترحيل' }).first().click();
    await expect(page.getByRole('dialog')).toBeVisible();
    await shoot(page, `ar-dark-post-dialog.png`);
  });
});
