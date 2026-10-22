/**
 * Settings, end to end against the fake API: each section reads its data,
 * saves through its command, shows refusals inline, and asks before the
 * consequential actions (closing a year). An agent key gets the explanation
 * instead of the forms. Screenshots of every section go to app/screenshots/.
 */
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test, type Page } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';
import { mockApi, signInPerson, signIn, stopMockApi } from './mockApi';
import { settingsState } from './mockSettings';

const here = path.dirname(fileURLToPath(import.meta.url));
const outDir = path.resolve(here, '../../screenshots');

test.afterAll(stopMockApi);

/** Chooses an option in a shared Select, by the visible label it is named by. */
async function pick(page: Page, label: string, option: string) {
  // The trigger is named by its label and then its current value ("Who Any").
  await page.getByRole('combobox', { name: new RegExp(`^${label}\\b`) }).click();
  await page.getByRole('option', { name: option, exact: true }).click();
}

/** Opens a Settings address and waits for its navigation (the words change with the locale). */
async function open(page: Page, hash: string) {
  await page.goto(`/${hash}`);
  await expect(page.locator('.settings-nav')).toBeVisible();
}

test.describe('settings', () => {
  test('the left navigation lists every section and marks the current one', async ({ page }) => {
    await mockApi({ person: true });
    await signInPerson(page);
    await open(page, '#/settings/tax');
    const nav = page.getByRole('navigation', { name: 'Settings sections' });
    for (const name of ['Company', 'Accounting year', 'Numbering', 'Tax', 'Approvals and agent rules', 'Invoice reading', 'Audit log', 'Agent keys']) {
      await expect(nav.getByRole('link', { name })).toBeVisible();
    }
    await expect(nav.getByRole('link', { name: 'Tax' })).toHaveAttribute('aria-current', 'page');
    await expect(page.getByRole('heading', { name: 'VAT rules' })).toBeVisible();
  });

  test('company: a bad VAT ID is refused inline; a good one saves and is audited', async ({ page }) => {
    await mockApi({ person: true });
    await signInPerson(page);
    await open(page, '#/settings/company');
    await page.getByLabel('VAT ID', { exact: true }).fill('FR123456789');
    await page.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(page.getByText('The VAT ID must start with the country code.')).toBeVisible();
    await expect(page.getByLabel('VAT ID', { exact: true })).toHaveAttribute('aria-invalid', 'true');

    await page.getByLabel('VAT ID', { exact: true }).fill('de 123.456.789');
    await page.getByLabel('Contact email', { exact: true }).fill('not-an-address');
    await page.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(page.getByText('Enter one email address')).toBeVisible();

    await page.getByLabel('Contact email', { exact: true }).fill('buchhaltung@halden.example');
    await page.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(page.getByText('Company details saved.').first()).toBeVisible();
    await expect(page.getByLabel('VAT ID', { exact: true })).toHaveValue('DE123456789');
  });

  test('company: the base currency is read only and says why', async ({ page }) => {
    await mockApi({ person: true });
    await signInPerson(page);
    await open(page, '#/settings/company');
    await expect(page.getByText('Open: no entries yet')).toBeVisible();
    await expect(page.getByRole('button', { name: /change currency/i })).toHaveCount(0);
    await expect(page.getByText('Every report and the year-end close read the base currency')).toBeVisible();
  });

  test('years: open the next year, lock a month, reopen it with a reason, and close the year after a confirm', async ({ page }) => {
    await mockApi({ person: true });
    await signInPerson(page);
    await open(page, '#/settings/years');
    await page.getByRole('button', { name: 'Open 2027' }).click();
    await expect(page.getByText('Opened fiscal year 2027.')).toBeVisible();
    await expect(page.getByRole('heading', { name: 'Fiscal year 2027' })).toBeVisible();

    await page.getByRole('button', { name: /^Lock .* 2026$/ }).first().click();
    await expect(page.getByText('Locked Jan 2026.')).toBeVisible();

    await page.getByRole('button', { name: /^Reopen .* 2026$/ }).first().click();
    const reopen = page.getByRole('dialog', { name: /Reopen/ });
    const confirm = reopen.getByRole('button', { name: 'Reopen', exact: true });
    await expect(confirm).toBeDisabled();
    await reopen.getByLabel('Reason', { exact: true }).fill('Late supplier invoice');
    await confirm.click();
    await expect(page.getByText('Reopened Jan 2026.')).toBeVisible();

    await page.getByRole('button', { name: 'Close 2026' }).click();
    const dialog = page.getByRole('dialog', { name: 'Close fiscal year 2026?' });
    await expect(dialog).toBeVisible();
    await expect(dialog.getByText('This locks each month of 2026 before December')).toBeVisible();
    await expect(dialog.getByText('Every month before December is locked.')).toBeVisible();
    await dialog.getByRole('button', { name: 'Close 2026', exact: true }).click();
    await expect(page.getByText('Closed fiscal year 2026.')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Close 2026' })).toHaveCount(0);
  });

  test('numbering: the next number moves forward; a backward number is refused inline', async ({ page }) => {
    await mockApi({ person: true });
    await signInPerson(page);
    await open(page, '#/settings/numbering');
    const input = page.getByLabel('Next number for GN in 2026');
    await expect(input).toHaveValue('1');
    await input.fill('1');
    await page.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(page.getByText('The next number must be above 1.')).toBeVisible();

    await input.fill('50');
    await page.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(page.getByText('Next number for GN in 2026 saved.')).toBeVisible();
    await expect(page.getByLabel('Next number for GN in 2026')).toHaveValue('50');
  });

  test('tax: German rates are shown, and the basis and small-business rule save', async ({ page }) => {
    await mockApi({ person: true });
    await signInPerson(page);
    await open(page, '#/settings/tax');
    await expect(page.getByRole('cell', { name: '19%' })).toBeVisible();
    await expect(page.getByRole('cell', { name: '7%' })).toBeVisible();
    await page.getByLabel(/Cash \(VAT on payment date\)/).check();
    await page.getByLabel(/Small-business exemption/).check();
    await page.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(page.getByText('Tax settings saved.')).toBeVisible();
    await page.reload();
    await expect(page.getByLabel(/Small-business exemption/)).toBeChecked();
    await expect(page.getByLabel(/Cash \(VAT on payment date\)/)).toBeChecked();
  });

  test('approvals: a currency row validates, a row can be added and removed, and the expiry saves', async ({ page }) => {
    await mockApi({ person: true });
    await signInPerson(page);
    await open(page, '#/settings/approvals');
    await page.getByLabel('Amount, row 1').fill('100.505');
    await page.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(page.getByText('Enter a positive amount with at most the currency')).toBeVisible();
    await page.getByLabel('Amount, row 1').fill('250');

    await page.getByRole('button', { name: 'Add currency' }).click();
    await page.getByLabel('Currency, row 4').fill('JPY');
    await page.getByLabel('Amount, row 4').fill('30000');
    await page.getByLabel('Request expiry (hours)').fill('48');
    await page.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(page.getByText('Approval rules saved.')).toBeVisible();

    await page.getByRole('button', { name: 'Remove GBP' }).click();
    await expect(page.getByLabel('Currency, row 2')).toHaveCount(0);
    await expect(page.getByText('A high-risk approval needs a sign-in within the last 5 minutes.')).toBeVisible();
    await expect(page.getByRole('link', { name: 'Open agent keys' })).toHaveAttribute('href', '#/settings/agent-keys');
  });

  test('invoice reading: says which reader reads photos and what is off, in plain words', async ({ page }) => {
    await mockApi({ person: true });
    await signInPerson(page);
    await open(page, '#/settings/invoices');
    await expect(page.getByText('Photos are read with MiniMax.')).toBeVisible();
    await expect(page.getByText('10 MB per file')).toBeVisible();
    await expect(page.getByText('50 pages')).toBeVisible();
    await expect(page.getByText('Everything optional is on.')).toBeVisible();
  });

  test('invoice reading: a missing key says what to add and which reader is used meanwhile', async ({ page }) => {
    await mockApi({ person: true });
    settingsState.status = {
      ok: true,
      extraction: { provider: 'rules', configured: 'minimax', reason: 'missing_key' },
      unavailable: [],
      limits: { document_max_bytes: 10 * 1024 * 1024, pdf_max_pages: 50 },
    };
    await signInPerson(page);
    await open(page, '#/settings/invoices');
    await expect(page.getByText('Not set up: add the minimax key in the server settings. Photos are read with the built-in rules, without a model.')).toBeVisible();
  });

  test('audit log: filters by who, outcome and command, and exports the filtered rows as CSV', async ({ page }) => {
    await mockApi({ person: true });
    await signInPerson(page);
    await open(page, '#/settings/audit');
    await expect(page.getByRole('row')).toHaveCount(9);
    await pick(page, 'Who', 'Agent key');
    await pick(page, 'Outcome', 'Needs approval');
    await page.getByRole('button', { name: 'Apply filters' }).click();
    await expect(page.getByRole('row')).toHaveCount(2);
    await expect(page.getByText('1200.00 exceeds per-entry limit')).toBeVisible();

    await pick(page, 'Who', 'Any');
    await pick(page, 'Outcome', 'Any');
    await page.getByLabel('Command', { exact: true }).fill('entries.post');
    await page.getByRole('button', { name: 'Apply filters' }).click();
    await expect(page.getByRole('row')).toHaveCount(5);

    await page.getByLabel('Command', { exact: true }).fill('');
    await page.getByRole('button', { name: 'Apply filters' }).click();
    const download = page.waitForEvent('download');
    await page.getByRole('button', { name: 'Export CSV' }).click();
    const file = await download;
    expect(file.suggestedFilename()).toMatch(/^ledgers-audit-\d{4}-\d{2}-\d{2}\.csv$/);
    const body = fs.readFileSync(await file.path(), 'utf8');
    expect(body.split('\r\n')[0]).toBe('seq,at,actor_kind,actor_id,command,outcome,objects,reason,request_id');
    expect(body.split('\r\n').filter(Boolean)).toHaveLength(9);
  });

  test('an agent key gets the explanation, not the forms', async ({ page }) => {
    await mockApi({ seed: true });
    await signIn(page);
    await open(page, '#/settings/company');
    await expect(page.getByText('Sign in as a person to change settings')).toBeVisible();
    await expect(page.getByLabel('Company name', { exact: true })).toHaveCount(0);
  });

  test('every section passes the accessibility checks', async ({ page }) => {
    await mockApi({ person: true });
    await signInPerson(page);
    for (const section of ['company', 'years', 'numbering', 'tax', 'approvals', 'invoices', 'audit']) {
      await open(page, `#/settings/${section}`);
      await page.waitForLoadState('networkidle');
      const results = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa']).analyze();
      expect(results.violations, `${section}: ${results.violations.map((v) => v.id).join(', ')}`).toEqual([]);
    }
  });
});

const screens = ['company', 'years', 'numbering', 'tax', 'approvals', 'invoices', 'audit'] as const;
const shots = [
  { locale: 'en', theme: 'dark' },
  { locale: 'en', theme: 'light' },
  { locale: 'ar', theme: 'dark' },
] as const;
const viewports = [
  { label: 'desktop', width: 1440, height: 900 },
  { label: 'phone', width: 390, height: 844 },
] as const;

test.describe('settings screenshots', () => {
  for (const shot of shots) {
    for (const viewport of viewports) {
      test(`${shot.locale} ${shot.theme} ${viewport.label}`, async ({ page }) => {
        await page.setViewportSize({ width: viewport.width, height: viewport.height });
        await mockApi({ person: true });
        await signInPerson(page, shot.theme, shot.locale);
        for (const section of screens) {
          await open(page, `#/settings/${section}`);
          await page.waitForLoadState('networkidle');
          // A fixed bottom bar would sit mid-page in a full-page capture; lay it out in flow for the picture.
          await page.addStyleTag({ content: '.bottom-nav { position: static !important; margin-top: 24px; }' });
          await page.screenshot({ path: path.join(outDir, `settings-${section}-${shot.locale}-${shot.theme}-${viewport.label}.png`), fullPage: true });
        }
      });
    }
  }
});
