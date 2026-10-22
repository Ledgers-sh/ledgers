/**
 * Writes screenshots of the bank screens for review: the accounts list and the
 * account page (with the reconcile panel open) at desktop 1440 and phone 390,
 * in dark and light; the account page in Arabic (right-to-left); the import
 * dialog and the CSV column wizard; and the rules page. Rendering only; the
 * axe checks live in bank.spec.ts. Written to BANK_SHOT_DIR, or
 * app/tests/bank-screenshots.
 */
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test, type Page } from '@playwright/test';
import { mockApi, signInPerson, stopMockApi } from './mockApi';

const here = path.dirname(fileURLToPath(import.meta.url));
const outDir = process.env.BANK_SHOT_DIR ?? path.resolve(here, '../bank-screenshots');

test.afterAll(stopMockApi);

const csvSample = 'Buchungstag;Empfaenger;Betrag;Verwendungszweck\n05.10.2026;Halden Supplies;-19,95;Rent October\n06.10.2026;Client;1.250,00;Fee\n';

/** A fixed bottom bar would sit mid-page in a full-page capture; lay it out in flow for the picture. */
async function shoot(page: Page, file: string, fullPage = true) {
  await page.waitForLoadState('networkidle');
  await page.addStyleTag({ content: '.bottom-nav { position: static !important; margin-top: 24px; }' });
  await page.screenshot({ path: path.join(outDir, file), fullPage });
}

async function openAccount(page: Page, hashId: string, locale: string) {
  await page.goto('/#/bank');
  await page.getByRole('link', { name: /Girokonto Halden/ }).click();
  await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
  // The reconcile panel opens on a line with suggestions.
  await page.locator('.bank-line').first().click();
  await expect(page.locator('#bank-reconcile .bank-suggestion').first()).toBeVisible();
  void hashId;
  void locale;
}

const viewports = [
  { label: 'desktop', width: 1440, height: 900 },
  { label: 'phone', width: 390, height: 844 },
] as const;

for (const viewport of viewports) {
  for (const theme of ['dark', 'light'] as const) {
    test.describe(`${viewport.label} ${theme}`, () => {
      test.use({ viewport: { width: viewport.width, height: viewport.height } });
      const prefix = `${viewport.label}-${theme}`;

      test('accounts list', async ({ page }) => {
        await mockApi({ seed: true, person: true, bank: true });
        await signInPerson(page, theme, 'en');
        await page.goto('/#/bank');
        await expect(page.getByRole('heading', { name: 'Bank', level: 1 })).toBeVisible();
        await shoot(page, `bank-${prefix}-accounts.png`);
      });

      test('account page with reconcile', async ({ page }) => {
        await mockApi({ seed: true, person: true, bank: true });
        await signInPerson(page, theme, 'en');
        await openAccount(page, '', 'en');
        await shoot(page, `bank-${prefix}-account-reconcile.png`);
      });
    });
  }
}

test.describe('desktop dark extras', () => {
  test.use({ viewport: { width: 1440, height: 900 } });

  test('account page in Arabic, right to left', async ({ page }) => {
    await mockApi({ seed: true, person: true, bank: true });
    await signInPerson(page, 'dark', 'ar');
    await openAccount(page, '', 'ar');
    await expect(page.locator('main#main')).toHaveAttribute('dir', 'rtl');
    await shoot(page, 'bank-desktop-dark-ar-account.png');
  });

  test('import dialog', async ({ page }) => {
    await mockApi({ seed: true, person: true, bank: true });
    await signInPerson(page, 'dark', 'en');
    await openAccount(page, '', 'en');
    await page.locator('main .page-head .btn-primary').click();
    await expect(page.locator('dialog.bank-import')).toBeVisible();
    await page.waitForLoadState('networkidle');
    await page.locator('dialog.bank-import').screenshot({ path: path.join(outDir, 'bank-desktop-dark-import.png') });
  });

  test('CSV column wizard', async ({ page }) => {
    await mockApi({ seed: true, person: true, bank: true });
    await signInPerson(page, 'dark', 'en');
    await openAccount(page, '', 'en');
    await page.locator('main .page-head .btn-primary').click();
    const dialog = page.locator('dialog.bank-import');
    await dialog.locator('input[type=file]').setInputFiles({ name: 'export.csv', mimeType: 'text/csv', buffer: Buffer.from(csvSample) });
    await expect(dialog.locator('.bank-preview')).toContainText('2026-10-05');
    await page.waitForLoadState('networkidle');
    await dialog.screenshot({ path: path.join(outDir, 'bank-desktop-dark-csv-wizard.png') });
  });

  test('rules', async ({ page }) => {
    await mockApi({ seed: true, person: true, bank: true });
    await signInPerson(page, 'dark', 'en');
    await page.goto('/#/bank/rules');
    await expect(page.getByRole('row', { name: /Hosting/ })).toBeVisible();
    await shoot(page, 'bank-desktop-dark-rules.png');
  });
});

test.describe('phone arabic', () => {
  test.use({ viewport: { width: 390, height: 844 } });
  test('accounts list in Arabic', async ({ page }) => {
    await mockApi({ seed: true, person: true, bank: true });
    await signInPerson(page, 'light', 'ar');
    await page.goto('/#/bank');
    await expect(page.getByRole('link', { name: /Girokonto Halden/ })).toBeVisible();
    await shoot(page, 'bank-phone-light-ar-accounts.png');
  });
});
