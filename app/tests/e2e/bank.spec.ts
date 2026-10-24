/**
 * The Bank screens against the fake API: the accounts list, an account's lines
 * with filters and paging, reconciling a line (match, rule, ignore), the
 * statement import (camt.053 and a CSV with its column wizard), the rules
 * list, and creating an account. Also axe checks on the bank pages.
 */
import AxeBuilder from '@axe-core/playwright';
import { expect, test, type Page } from '@playwright/test';
import { choose } from './select';
import { bankMock } from './mockBank';
import { mockApi, signIn, signInPerson, stopMockApi } from './mockApi';

test.afterAll(stopMockApi);

const ACCOUNT = /Girokonto Halden/;

async function openAccount(page: Page) {
  await page.goto('/#/bank');
  await page.getByRole('link', { name: ACCOUNT }).click();
  await expect(page.getByRole('heading', { name: 'Girokonto Halden', level: 1 })).toBeVisible();
}

test.describe('bank: accounts list', () => {
  test('lists the account with its IBAN in groups, balances and the difference, and offers no bank sync', async ({ page }) => {
    await mockApi({ seed: true, person: true, bank: true });
    await signInPerson(page);
    await page.goto('/#/bank');
    await expect(page.getByRole('heading', { name: 'Bank', level: 1 })).toBeVisible();
    const card = page.getByRole('link', { name: ACCOUNT });
    await expect(card).toContainText('DE89 3704 0044 0532 0130 00');
    await expect(card).toContainText('5,041.75');
    await expect(card).toContainText('3,800.00');
    await expect(card).toContainText('Not reconciled');
    await expect(page.getByRole('button', { name: /sync/i })).toHaveCount(0);
    await expect(page.getByText('Automatic bank feeds come with Ledgers Cloud.')).toBeVisible();
  });

  test('an empty book shows the create form to a person', async ({ page }) => {
    await mockApi({ seed: true, person: true, bank: false });
    await signInPerson(page);
    await page.goto('/#/bank');
    await expect(page.getByRole('heading', { name: 'No bank accounts yet', level: 2 })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'New bank account', level: 2 })).toBeVisible();
    await page.getByLabel('Name', { exact: true }).fill('Main account');
    await page.getByLabel('IBAN', { exact: true }).fill('DE88370400440532013000');
    await page.getByLabel('IBAN', { exact: true }).blur();
    await expect(page.locator('#bank-iban-error')).toContainText('check digits');
    await page.getByLabel('IBAN', { exact: true }).fill('DE89 3704 0044 0532 0130 00');
    await page.getByLabel('Currency', { exact: true }).fill('EUR');
    await choose(page, 'Ledger account', /Cash/);
    await page.getByLabel('Opening balance', { exact: true }).fill('1250,50');
    await page.getByRole('button', { name: 'Create account' }).click();
    await expect(page.locator('#bank-opening-error')).toBeVisible();
    await page.getByLabel('Opening balance', { exact: true }).fill('1250.50');
    await page.getByRole('button', { name: 'Create account' }).click();
    await expect(page.getByRole('heading', { name: 'Main account', level: 1 })).toBeVisible();
    expect(bankMock.accounts.some((a) => a.name === 'Main account' && a.opening.minor === '125050')).toBe(true);
  });

  test('an agent key sees the accounts but no create form', async ({ page }) => {
    await mockApi({ seed: true, bank: true });
    await signIn(page);
    await page.goto('/#/bank');
    await expect(page.getByRole('link', { name: ACCOUNT })).toBeVisible();
    await expect(page.getByRole('link', { name: 'New bank account' })).toHaveCount(0);
  });
});

test.describe('bank: account page', () => {
  test('filters by status, search text and dates, and pages with Load more', async ({ page }) => {
    await mockApi({ seed: true, person: true, bank: true });
    await signInPerson(page);
    await openAccount(page);
    const lines = page.locator('.bank-line');
    await expect(lines).toHaveCount(50);
    await page.getByRole('button', { name: 'Load more' }).click();
    await expect(lines).toHaveCount(61);
    await expect(page.getByRole('button', { name: 'Load more' })).toHaveCount(0);

    await choose(page, 'Status', 'Suggested');
    await expect(lines).toHaveCount(3);
    await expect(page.getByRole('button', { name: /Halden Supplies GmbH/ })).toBeVisible();

    await choose(page, 'Status', 'All');
    await page.getByRole('searchbox', { name: 'Search' }).fill('Telekom');
    await expect(lines).toHaveCount(1);
    await expect(lines.first()).toContainText('Telekom Deutschland GmbH');

    await page.getByRole('searchbox', { name: 'Search' }).fill('');
    await page.getByLabel('From', { exact: true }).fill('2026-10-04');
    await page.getByLabel('To', { exact: true }).fill('2026-10-05');
    await expect(lines).toHaveCount(2);
  });

  test('reconciles a line by matching its suggested bill, with the posting shown', async ({ page }) => {
    await mockApi({ seed: true, person: true, bank: true });
    await signInPerson(page);
    await openAccount(page);
    await page.getByRole('button', { name: /Halden Supplies GmbH/ }).click();
    const panel = page.locator('#bank-reconcile');
    await expect(panel.getByRole('meter', { name: 'Confidence' }).first()).toHaveAttribute('aria-valuenow', '92');
    await expect(panel.getByText('Same amount · Invoice number found in the remittance · The IBAN is the vendor\'s')).toBeVisible();
    await expect(panel.getByRole('table')).toContainText('Accounts payable');
    await expect(panel.getByText(/remittance text|Text from the bank/).first()).toBeVisible();
    await panel.getByRole('button', { name: 'Match', exact: true }).click();
    await expect(page.getByText('Line matched.')).toBeVisible();
    expect(bankMock.writes).toContain('post:tx_bill:bill_open_1');
    await expect(page.getByRole('button', { name: /Halden Supplies GmbH/ })).toContainText('Matched');
  });

  test('a customer receipt settles the chosen customer\'s open invoices, oldest first, then posts', async ({ page }) => {
    await mockApi({ seed: true, person: true, bank: true });
    await signInPerson(page);
    await openAccount(page);
    await page.getByRole('button', { name: /Client Berlin AG/ }).click();
    const panel = page.locator('#bank-reconcile');
    await panel.getByRole('radio', { name: 'Another account' }).click();
    await choose(page, 'Other account', /Accounts receivable/);

    const heading = panel.getByRole('heading', { name: 'Customer receipt' });
    await expect(heading).toBeVisible();
    await page.getByRole('combobox', { name: 'Customer' }).fill('Acme');
    await page.getByRole('option', { name: 'Acme GmbH' }).click();

    // The receipt of 2500.00 settles both invoices exactly: 1000.00, then 1500.00.
    await expect(panel.getByText('RE-2026-0101')).toBeVisible();
    await expect(panel.getByText('RE-2026-0108')).toBeVisible();
    await expect(panel.locator('.receipt-item', { hasText: 'RE-2026-0101' })).toContainText('1,000.00');
    await expect(panel.locator('.receipt-item', { hasText: 'RE-2026-0108' })).toContainText('1,500.00');
    await expect(panel.getByText('On account')).toHaveCount(0);

    await panel.getByRole('button', { name: 'Match', exact: true }).click();
    await expect(page.getByText('Line matched.')).toBeVisible();
    expect(bankMock.writes).toContain('receipt:tx_client:party-acme');
  });

  test('shows the journal a rule would post, and posts it to the rule account', async ({ page }) => {
    await mockApi({ seed: true, person: true, bank: true });
    await signInPerson(page);
    await openAccount(page);
    await page.getByRole('button', { name: /Telekom Deutschland/ }).click();
    const panel = page.locator('#bank-reconcile');
    await expect(panel.getByText('Rule', { exact: true })).toBeVisible();
    await expect(panel.getByRole('table')).toContainText('6100');
    await panel.getByRole('button', { name: 'Match', exact: true }).click();
    await expect(page.getByText('Line matched.')).toBeVisible();
    expect(bankMock.writes).toContain('post:tx_telekom:6100');
  });

  test('ignores a line on purpose', async ({ page }) => {
    await mockApi({ seed: true, person: true, bank: true });
    await signInPerson(page);
    await openAccount(page);
    await page.getByRole('button', { name: /Telekom Deutschland/ }).click();
    await page.locator('#bank-reconcile').getByRole('button', { name: 'Ignore line' }).click();
    await expect(page.getByText('Line ignored.')).toBeVisible();
    expect(bankMock.writes).toContain('ignore:tx_telekom');
  });

  test('a remittance that reads like an instruction is flagged and never acted on', async ({ page }) => {
    await mockApi({ seed: true, person: true, bank: true });
    await signInPerson(page);
    await openAccount(page);
    await page.getByRole('button', { name: /Unbekannt/ }).click();
    await expect(page.locator('#bank-reconcile').getByRole('note')).toContainText('never acted on');
    await expect(page.locator('#bank-reconcile').getByRole('button', { name: 'Match', exact: true })).toBeDisabled();
  });

  test('an agent key posts for approval, and the panel says so', async ({ page }) => {
    await mockApi({ seed: true, bank: true });
    await signIn(page);
    await openAccount(page);
    await page.getByRole('button', { name: /Halden Supplies GmbH/ }).click();
    await page.locator('#bank-reconcile').getByRole('button', { name: 'Match', exact: true }).click();
    await expect(page.getByText('Sent for approval.')).toBeVisible();
    await expect(page.getByRole('link', { name: 'Open the approval' })).toHaveAttribute('href', /#\/approvals\//);
  });

  test('creates a rule from a line; a person’s rule applies at once', async ({ page }) => {
    await mockApi({ seed: true, person: true, bank: true });
    await signInPerson(page);
    await openAccount(page);
    await page.getByRole('button', { name: /Client Berlin AG/ }).click();
    const panel = page.locator('#bank-reconcile');
    await panel.getByRole('button', { name: 'Create rule' }).click();
    await expect(panel.getByLabel('Remittance or name contains')).toHaveValue('Honorar');
    await choose(page, 'Account', /6100/);
    await panel.getByRole('button', { name: 'Save rule' }).click();
    await expect(page.getByText('Rule saved and applied.')).toBeVisible();
    expect(bankMock.rules.some((rule) => rule.name === 'Honorar' && rule.enabled)).toBe(true);
  });
});

test.describe('bank: statement import', () => {
  test('imports a camt.053 statement and shows the summary with the balance checked', async ({ page }) => {
    await mockApi({ seed: true, person: true, bank: true });
    await signInPerson(page);
    await openAccount(page);
    await page.getByRole('button', { name: 'Import statement' }).click();
    const dialog = page.locator('dialog.bank-import');
    await expect(dialog).toBeVisible();
    await dialog.locator('input[type=file]').setInputFiles({
      name: 'october.xml',
      mimeType: 'application/xml',
      buffer: Buffer.from('<?xml version="1.0"?><Document xmlns="urn:iso:std:iso:20022:tech:xsd:camt.053.001.02"><BkToCstmrStmt></BkToCstmrStmt></Document>'),
    });
    await expect(dialog.getByRole('combobox', { name: 'File format' })).toContainText('camt.053');
    await dialog.getByRole('button', { name: 'Import', exact: true }).click();
    await expect(dialog.getByText('Import summary')).toBeVisible();
    await expect(dialog).toContainText('3 new lines, 0 already there.');
    await expect(dialog).toContainText('Yes');
    await dialog.getByRole('button', { name: 'Done' }).click();
    expect(bankMock.writes.some((w) => w.startsWith('import:'))).toBe(true);
  });

  test('a CSV needs its column mapping first; the wizard previews rows and saves the mapping', async ({ page }) => {
    await mockApi({ seed: true, person: true, bank: true });
    await signInPerson(page);
    await openAccount(page);
    await page.getByRole('button', { name: 'Import statement' }).click();
    const dialog = page.locator('dialog.bank-import');
    await dialog.locator('input[type=file]').setInputFiles({
      name: 'export.csv',
      mimeType: 'text/csv',
      buffer: Buffer.from('Buchungstag;Empfaenger;Betrag;Verwendungszweck\n05.10.2026;Halden Supplies;-19,95;Rent October\n06.10.2026;Client;1.250,00;Fee\n'),
    });
    await expect(dialog.getByRole('combobox', { name: 'File format' })).toContainText('CSV');
    await expect(dialog.getByRole('heading', { name: 'Column mapping' })).toBeVisible();
    const preview = dialog.locator('.bank-preview');
    await expect(preview).toContainText('2026-10-05');
    await expect(preview).toContainText('−19.95');
    await expect(preview).toContainText('+1,250.00');
    await expect(dialog.getByRole('button', { name: 'Import', exact: true })).toBeDisabled();
    await dialog.getByRole('button', { name: 'Save column mapping' }).click();
    await expect(dialog.getByText('A column mapping is saved for this account.')).toBeVisible();
    await expect(page.getByText('Column mapping saved.')).toBeVisible();
    await dialog.getByRole('button', { name: 'Import', exact: true }).click();
    await expect(dialog.getByText('Import summary')).toBeVisible();
    await expect(dialog).toContainText('Balances checked');
    await expect(dialog).toContainText('No (a CSV file has no balances)');
    expect(bankMock.accounts[0].csv_mapping).not.toBeNull();
  });
});

test.describe('bank: rules', () => {
  test('a person enables a proposed rule; the button is gone after', async ({ page }) => {
    await mockApi({ seed: true, person: true, bank: true });
    await signInPerson(page);
    await page.goto('/#/bank/rules');
    await expect(page.getByRole('heading', { name: 'Bank rules', level: 1 })).toBeVisible();
    const row = page.getByRole('row', { name: /Hosting/ });
    await expect(row).toContainText('Proposed');
    await row.getByRole('button', { name: 'Enable' }).click();
    await expect(page.getByText('Rule enabled.')).toBeVisible();
    expect(bankMock.writes).toContain('enable:rule_hosting');
    await expect(page.getByRole('row', { name: /Hosting/ }).getByRole('button', { name: 'Enable' })).toHaveCount(0);
  });

  test('an agent key sees proposed rules without an enable button', async ({ page }) => {
    await mockApi({ seed: true, bank: true });
    await signIn(page);
    await page.goto('/#/bank/rules');
    await expect(page.getByRole('row', { name: /Hosting/ })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Enable' })).toHaveCount(0);
  });
});

test.describe('bank: Arabic and accessibility', () => {
  test('the account page lays out right to left in Arabic', async ({ page }) => {
    await mockApi({ seed: true, person: true, bank: true });
    await signInPerson(page, 'dark', 'ar');
    await openAccount(page);
    await expect(page.locator('main#main')).toHaveAttribute('dir', 'rtl');
  });

  for (const locale of ['en', 'de', 'ar'] as const) {
    test(`axe finds no serious problems on the bank pages (${locale})`, async ({ page }) => {
      await mockApi({ seed: true, person: true, bank: true });
      await signInPerson(page, 'dark', locale);
      const blocking = async () => {
        const results = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa']).analyze();
        return results.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical').map((v) => `${v.impact} ${v.id}: ${v.help}`);
      };
      await page.goto('/#/bank');
      await expect(page.getByRole('link', { name: ACCOUNT })).toBeVisible();
      await page.waitForLoadState('networkidle');
      expect(await blocking()).toEqual([]);

      await page.getByRole('link', { name: ACCOUNT }).click();
      await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
      await page.locator('.bank-line').first().click();
      await page.waitForLoadState('networkidle');
      expect(await blocking()).toEqual([]);

      // The header's first button opens the import (the labels differ per language, so the test does not name it).
      await page.locator('main .page-head .btn-primary').click();
      await expect(page.locator('dialog.bank-import')).toBeVisible();
      expect(await blocking()).toEqual([]);
      await page.keyboard.press('Escape');

      await page.goto('/#/bank/rules');
      await expect(page.locator('table.bank-rules tbody tr')).toHaveCount(2);
      await page.waitForLoadState('networkidle');
      expect(await blocking()).toEqual([]);
    });
  }
});
