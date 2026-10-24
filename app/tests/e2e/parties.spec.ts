/**
 * Customers and vendors against the fake API: the list and its filters, the
 * create form's inline checks, the bank-details confirm (owners and admins
 * only), the merge preview and the server's refusal, and axe on the screens.
 */
import AxeBuilder from '@axe-core/playwright';
import { expect, test, type Page } from '@playwright/test';
import { choose } from './select';
import { mockApi, signInPerson, stopMockApi } from './mockApi';
import { partiesState } from './mockParties';
import { peopleState } from './mockPeople';

test.afterAll(stopMockApi);

const rows = (page: Page) => page.locator('table.parties-table tbody tr');

async function openList(page: Page) {
  await page.goto('/#/parties');
  await expect(rows(page).first()).toBeVisible();
}

async function blockingViolations(page: Page) {
  const results = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa']).analyze();
  return results.violations
    .filter((v) => v.impact === 'serious' || v.impact === 'critical')
    .map((v) => `${v.impact} ${v.id}: ${v.help} (${v.nodes.length} nodes)`);
}

test.describe('customers and vendors', () => {
  test.beforeEach(async () => {
    await mockApi({ person: true });
  });

  test('the list hides merged parties, filters by role and review, and searches by name or VAT ID', async ({ page }) => {
    await signInPerson(page);
    await openList(page);
    await expect(rows(page)).toHaveCount(4);
    await expect(page.locator('table.parties-table')).not.toContainText('Halden Works (old)');

    await choose(page, 'Role', /^Customer$/);
    await expect(rows(page)).toHaveCount(1);
    await expect(rows(page).first()).toContainText('Acme GmbH');

    await choose(page, 'Role', 'All roles');
    await choose(page, 'Review', 'Needs review');
    await expect(rows(page)).toHaveCount(2);

    await choose(page, 'Review', 'Any review state');
    await page.getByPlaceholder('Name, VAT ID or IBAN').fill('CHE123');
    await expect(rows(page)).toHaveCount(1);
    await expect(rows(page).first()).toContainText('Lumen Logistik AG');

    await page.getByPlaceholder('Name, VAT ID or IBAN').fill('zzz-nothing');
    await expect(page.getByRole('heading', { name: 'No party matches' })).toBeVisible();
  });

  test('the create form checks the VAT ID as the server does, blocks save, and creates once it passes', async ({ page }) => {
    await signInPerson(page);
    await page.goto('/#/parties/new');
    await page.getByLabel('Legal name').fill('Nordwind Trading GmbH');
    const vat = page.getByLabel('VAT ID', { exact: true });
    await vat.fill('DE12');
    await vat.blur();
    await expect(page.locator('#party-vat-error')).toContainText("does not pass its country's check");
    await expect(vat).toHaveAttribute('aria-invalid', 'true');

    await page.getByRole('button', { name: 'Create party' }).click();
    expect(partiesState.requests).not.toContain('POST /v1/parties');
    await expect(page.locator('#party-vat-error')).toBeVisible();

    await vat.fill('DE136695976');
    await vat.blur();
    await expect(page.locator('#party-vat-error')).toContainText('already has this VAT ID');

    await vat.fill('DE811907154');
    await vat.blur();
    await expect(page.locator('#party-vat-error')).toHaveCount(0);
    await page.getByRole('button', { name: 'Create party' }).click();
    await expect(page.getByRole('heading', { name: 'Nordwind Trading GmbH' })).toBeVisible();
    expect(partiesState.requests.filter((r) => r === 'POST /v1/parties')).toHaveLength(1);
  });

  test('an owner changes bank details only after the fraud and audit warning, and the history shows the change', async ({ page }) => {
    await signInPerson(page);
    await page.goto('/#/parties/party-halden');
    await expect(page.getByRole('heading', { name: 'Halden Works GmbH' })).toBeVisible();
    await page.getByRole('button', { name: 'Change bank details' }).click();

    const dialog = page.getByRole('dialog');
    await expect(dialog).toContainText('common fraud route');
    await expect(dialog).toContainText('audited');
    await expect(dialog).toContainText('Only you can make this change');

    await dialog.getByLabel('IBAN').fill('DE02 1203 0000 0000 2020 51');
    await dialog.getByLabel('Reason for the change').fill('Confirmed by phone with Lea');
    await dialog.getByRole('button', { name: 'Change bank details' }).click();

    await expect(dialog).toHaveCount(0);
    await expect(page.locator('ol[aria-label="History"]')).toContainText('Confirmed by phone with Lea');
    expect(partiesState.requests).toContain('POST /v1/parties/party-halden/set-bank');
  });

  test('an accountant sees the bank details but no way to change them', async ({ page }) => {
    peopleState.role = 'accountant';
    await signInPerson(page);
    await page.goto('/#/parties/party-halden');
    await expect(page.getByRole('heading', { name: 'Halden Works GmbH' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Change bank details' })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Merge into…' })).toHaveCount(0);
    await expect(page.getByText('Only an owner or an admin can change bank details.')).toBeVisible();
  });

  test('a merge shows what moves and what stays before it is confirmed', async ({ page }) => {
    await signInPerson(page);
    await page.goto('/#/parties/party-acme');
    await page.getByRole('button', { name: 'Merge into…' }).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByRole('combobox', { name: 'Merge into…' }).fill('Lumen');
    await dialog.getByRole('option', { name: 'Lumen Logistik AG' }).click();

    await expect(dialog.getByRole('heading', { name: 'What will happen' })).toBeVisible();
    await expect(dialog).toContainText('0 bills move to Lumen Logistik AG.');
    await expect(dialog).toContainText('Acme GmbH becomes read-only.');
    await expect(dialog).toContainText("This party's VAT ID stays on its merged record.");

    await dialog.getByRole('button', { name: 'Merge party' }).click();
    await expect(page.getByRole('heading', { name: 'Lumen Logistik AG' })).toBeVisible();
    expect(partiesState.requests).toContain('POST /v1/parties/party-lumen/merge');
  });

  test('a merge the server refuses says why, in the dialog', async ({ page }) => {
    partiesState.refuseMerge = 'source party is already merged';
    await signInPerson(page);
    await page.goto('/#/parties/party-acme');
    await page.getByRole('button', { name: 'Merge into…' }).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByRole('combobox', { name: 'Merge into…' }).fill('Lumen');
    await dialog.getByRole('option', { name: 'Lumen Logistik AG' }).click();
    await dialog.getByRole('button', { name: 'Merge party' }).click();
    await expect(dialog).toContainText('source party is already merged');
    await expect(page.getByRole('heading', { name: 'Acme GmbH' })).toBeVisible();
  });

  test('VAT checks say they are off when the server has them off', async ({ page }) => {
    partiesState.viesEnabled = false;
    await signInPerson(page);
    await page.goto('/#/parties/party-halden');
    await expect(page.getByText('VAT checks are off on this server.')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Check VAT ID' })).toHaveCount(0);
  });

  for (const [name, hash, locale] of [
    ['list', '#/parties', 'en'],
    ['party page', '#/parties/party-halden', 'en'],
    ['party form', '#/parties/new', 'en'],
    ['party page in Arabic', '#/parties/party-halden', 'ar'],
  ] as const) {
    test(`axe finds no serious violation on the ${name}`, async ({ page }) => {
      await page.addInitScript((code) => window.localStorage.setItem('ledgers.locale', code), locale);
      await signInPerson(page, 'dark', locale);
      await page.goto(`/${hash}`);
      await page.waitForLoadState('networkidle');
      expect(await blockingViolations(page)).toEqual([]);
    });
  }
});
