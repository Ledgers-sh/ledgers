/**
 * The party directory against the fake API: the detail page's metadata,
 * primary contact, open items and recent activity; a logo uploaded from the
 * edit page and shown in the list and on the page; one primary contact, with
 * the server's refusal in words; and a CSV file checked row by row before its
 * import can run. Also axe on the new screens, and screenshots at 390 and 1280
 * when PARTIES_SHOTS_DIR is set.
 */
import AxeBuilder from '@axe-core/playwright';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test, type Page } from '@playwright/test';
import { mockApi, signInPerson, stopMockApi } from './mockApi';
import { partiesState, resetParties } from './mockParties';

test.afterAll(stopMockApi);

const LOGO = fileURLToPath(new URL('../../../fixtures/demo/logos/ledgers.png', import.meta.url));
const SHOTS = process.env.PARTIES_SHOTS_DIR;

async function blockingViolations(page: Page) {
  const results = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa']).analyze();
  return results.violations
    .filter((v) => v.impact === 'serious' || v.impact === 'critical')
    .map((v) => `${v.impact} ${v.id}: ${v.help} (${v.nodes.length} nodes)`);
}

async function shot(page: Page, name: string) {
  if (!SHOTS) return;
  fs.mkdirSync(SHOTS, { recursive: true });
  await page.waitForLoadState('networkidle');
  await page.screenshot({ path: path.join(SHOTS, `${name}.png`), fullPage: true });
}

test.describe('the party directory', () => {
  test.beforeEach(async () => {
    resetParties();
    await mockApi({ person: true });
  });

  test('the detail page shows the metadata, the primary contact, the open items and the recent activity', async ({ page }) => {
    await page.goto('/#/parties/party-halden');
    await expect(page.getByRole('heading', { name: 'Halden Works GmbH' })).toBeVisible();
    await expect(page.getByText('billing@halden.example')).toBeVisible();
    await expect(page.getByText('wholesale', { exact: true })).toBeVisible();
    await expect(page.getByText('NL', { exact: true })).toBeVisible();
    await expect(page.getByText('Primary', { exact: true })).toBeVisible();
    const open = page.locator('section[aria-labelledby="party-open-title"]');
    await expect(open.getByText('HW-2026-114')).toBeVisible();
    await expect(open.getByText('Payable').first()).toBeVisible();
    await expect(page.getByRole('heading', { name: 'Recent payments' })).toBeVisible();
    expect(await blockingViolations(page)).toEqual([]);
    await shot(page, 'party-detail-desktop');
  });

  test('a logo is uploaded from the edit page and shows in the list and on the detail page', async ({ page }) => {
    await page.goto('/#/parties/party-halden/edit');
    await page.getByLabel('Logo (optional)').setInputFiles(LOGO);
    await expect(page.getByText('Logo saved.')).toBeVisible();
    expect(partiesState.parties.find((p) => p.id === 'party-halden')?.logo_document_id).toBeTruthy();

    await page.goto('/#/parties');
    await expect(page.locator('table.parties-table img.avatar-logo').first()).toBeVisible();

    await page.goto('/#/parties/party-halden');
    await expect(page.locator('img.avatar-logo').first()).toBeVisible();
    await shot(page, 'party-logo-desktop');
  });

  test('a second primary contact is refused in words, and the first is kept', async ({ page }) => {
    await page.goto('/#/parties/new');
    await page.getByLabel('Legal name').fill('Nordwind Trading GmbH');
    await page.getByRole('button', { name: 'Add contact' }).click();
    await page.locator('#party-c0-name').fill('Ada');
    await page.locator('#party-c0-primary').check();
    await page.getByRole('button', { name: 'Add contact' }).click();
    await page.locator('#party-c1-name').fill('Grace');
    await page.locator('#party-c1-primary').check();
    await page.getByRole('button', { name: 'Create party' }).click();
    await expect(page.getByText('Only one contact can be the primary contact.')).toBeVisible();
    expect(await blockingViolations(page)).toEqual([]);
  });

  test('a CSV file is checked row by row, and its import stays off until every row is valid', async ({ page }) => {
    await page.goto('/#/parties/import');
    await expect(page.getByRole('heading', { name: 'Import parties from CSV' })).toBeVisible();

    await page.locator('#party-import-file').setInputFiles({
      name: 'parties.csv',
      mimeType: 'text/csv',
      buffer: Buffer.from('id,name,vat_id,email\nnew-1,Good Ltd,,good@example.test\nnew-2,Bad VAT GmbH,XX123,\n'),
    });
    await page.getByRole('button', { name: 'Check file' }).click();
    await expect(page.getByText('1 to create, 0 to update, 1 with errors.')).toBeVisible();
    await expect(page.getByText("This VAT ID does not pass its country's check.")).toBeVisible();
    await expect(page.getByRole('button', { name: 'Import 2 rows' })).toBeDisabled();
    expect(await blockingViolations(page)).toEqual([]);
    await shot(page, 'party-import-errors-desktop');

    await page.locator('#party-import-file').setInputFiles({
      name: 'parties.csv',
      mimeType: 'text/csv',
      buffer: Buffer.from('id,name,vat_id,email\nnew-1,Good Ltd,,good@example.test\n'),
    });
    await page.getByRole('button', { name: 'Check file' }).click();
    await expect(page.getByText('Every row is valid.').first()).toBeVisible();
    await page.getByRole('button', { name: 'Import 1 rows' }).click();
    await expect(page.getByText('1 created, 0 updated.').first()).toBeVisible();
  });

  test('the export downloads the parties as CSV', async ({ page }) => {
    await page.goto('/#/parties');
    const download = page.waitForEvent('download');
    await page.getByRole('button', { name: 'Export CSV' }).click();
    const file = await download;
    expect(file.suggestedFilename()).toBe('parties.csv');
  });
});

test.describe('screenshots at 390 and 1280', () => {
  test.skip(!SHOTS, 'PARTIES_SHOTS_DIR is not set');
  test.beforeEach(async () => {
    resetParties();
    await mockApi({ person: true });
  });

  for (const [label, width] of [
    ['390', 390],
    ['1280', 1280],
  ] as const) {
    test(`directory, detail and import at ${label}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await signInPerson(page, 'light');
      await page.goto('/#/parties');
      await shot(page, `directory-${label}`);
      await page.goto('/#/parties/party-halden');
      await shot(page, `detail-${label}`);
      await page.goto('/#/parties/import');
      await shot(page, `import-${label}`);
      await page.locator('#party-import-file').setInputFiles({
        name: 'parties.csv',
        mimeType: 'text/csv',
        buffer: Buffer.from('id,name,vat_id,email\nnew-1,Good Ltd,,good@example.test\nnew-2,Bad VAT GmbH,XX123,\n'),
      });
      await page.getByRole('button', { name: 'Check file' }).click();
      await expect(page.getByText('Some rows have errors')).toHaveCount(0);
      await expect(page.getByText("This VAT ID does not pass its country's check.")).toBeVisible();
      await shot(page, `import-errors-${label}`);
    });
  }
});
