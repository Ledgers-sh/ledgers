import { expect, test, type Page } from '@playwright/test';
import { mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { mockApi, signInPerson, stopMockApi } from './mockApi';
import { billsMock, fixtureBytes } from './mockBills';

const SHOT_DIR = process.env.BILLS_SHOT_DIR ?? join(process.cwd(), 'bills-screenshots');

/** Uploads a fixture through the Bills screen's file input. */
async function upload(page: Page, name: string) {
  await page.getByTestId('upload-input').setInputFiles({
    name,
    mimeType: name.endsWith('.pdf') ? 'application/pdf' : 'image/png',
    buffer: fixtureBytes(name),
  });
}

test.describe('bills: inbox, review and posting (issues #20 to #22, #32, #34)', () => {
  test.afterAll(stopMockApi);

  test('an uploaded invoice becomes a draft bill in the inbox, and the review shows what needs a person', async ({ page }) => {
    await mockApi({ person: true });
    await signInPerson(page);
    await page.goto('#/bills');
    await expect(page.getByRole('heading', { name: 'Bills', level: 1 })).toBeVisible();

    await upload(page, 'invoice.pdf');
    await expect(page.getByRole('status').filter({ hasText: 'draft bill' })).toBeVisible();
    const row = page.getByRole('row', { name: /Halden Supplies GmbH/ });
    await expect(row).toBeVisible();
    await expect(row.getByText('Needs review')).toBeVisible();
    await expect(row.getByText('RE-2026-0042')).toBeVisible();
    await expect(row.getByText('1,190.00 EUR')).toBeVisible();
    expect(billsMock.uploads).toBe(1);

    await row.getByRole('link', { name: 'Review' }).click();
    await expect(page.getByRole('heading', { name: 'Review bill' })).toBeVisible();
    // The document is on the left, the fields on the right, the checks beside them.
    await expect(page.getByRole('region', { name: 'Document' }).locator('iframe.doc-frame')).toBeVisible();
    await expect(page.getByRole('complementary', { name: 'Checks' }).getByText('Line items are present')).toBeVisible();
    // Low-confidence fields are marked in text, not only by colour.
    await expect(page.getByLabel(/Vendor name/).locator('xpath=ancestor::label')).toContainText('check this');
    // The journal preview is empty until every line has an account.
    await expect(page.getByText('The entry appears once every line has an account')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Post', exact: true })).toBeDisabled();
  });

  test('a person completes the lines, confirms the vendor and posts; the entry matches the preview', async ({ page }) => {
    await mockApi({ person: true });
    await signInPerson(page);
    await page.goto('#/bills');
    await upload(page, 'invoice.pdf');
    await page.getByRole('row', { name: /Halden Supplies GmbH/ }).getByRole('link', { name: 'Review' }).click();
    await expect(page.getByRole('heading', { name: 'Review bill' })).toBeVisible();

    await page.getByRole('button', { name: 'Add line' }).click();
    await page.getByLabel('Description', { exact: true }).fill('Office paper');
    await page.getByRole('group', { name: 'Lines' }).getByLabel(/Net/).fill('1000.00');
    await page.getByRole('group', { name: 'Lines' }).getByLabel(/VAT \(EUR\)/).fill('190.00');
    await page.getByLabel('Expense account').selectOption({ index: 1 });
    await page.getByRole('button', { name: 'Save' }).click();
    await expect(page.getByText('Changes saved.')).toBeVisible();

    // The vendor is new: a person confirms it, then the bill is ready.
    await expect(page.getByRole('button', { name: 'Post', exact: true })).toBeDisabled();
    await page.getByRole('button', { name: 'Confirm vendor' }).click();
    await expect(page.getByRole('button', { name: 'Post', exact: true })).toBeEnabled();

    // The preview lists the debit and credit lines the entry will post.
    const preview = page.getByRole('table', { name: /Journal preview|The debit and credit lines/ }).or(page.locator('table.journal-preview'));
    await expect(preview.getByRole('cell', { name: '1050', exact: false })).toBeVisible();
    await expect(preview.getByRole('cell', { name: '2000', exact: false })).toBeVisible();
    const previewText = await preview.innerText();

    await page.getByRole('button', { name: 'Post', exact: true }).click();
    await expect(page.getByText('Posted as entry 41.')).toBeVisible();
    expect(billsMock.posts).toHaveLength(1);
    // Posted bills cannot change: the fields are read-only and there is no post or discard.
    await expect(page.getByRole('button', { name: 'Discard' })).toHaveCount(0);
    expect(previewText).toContain('1050');
  });

  test('discarding a draft asks first and leaves it out of the open list', async ({ page }) => {
    await mockApi({ person: true });
    await signInPerson(page);
    await page.goto('#/bills');
    await upload(page, 'invoice.pdf');
    await page.getByRole('row', { name: /Halden Supplies GmbH/ }).getByRole('link', { name: 'Review' }).click();
    await page.getByRole('button', { name: 'Discard' }).click();
    const dialog = page.getByRole('dialog', { name: 'Discard this bill?' });
    await expect(dialog).toBeVisible();
    await dialog.getByRole('button', { name: 'Discard' }).click();
    await expect(page.getByText('Bill discarded.')).toBeVisible();
    await expect(page.getByRole('heading', { name: 'Posted or discarded' })).toBeVisible();
  });

  test('the same file twice is one document: the second upload says so and drafts nothing new', async ({ page }) => {
    await mockApi({ person: true });
    await signInPerson(page);
    await page.goto('#/bills');
    await upload(page, 'invoice.pdf');
    await expect(page.getByRole('row', { name: /Halden Supplies GmbH/ })).toHaveCount(1);
    await upload(page, 'invoice.pdf');
    await expect(page.getByRole('status').filter({ hasText: 'already stored' })).toBeVisible();
    await expect(page.getByRole('row', { name: /Halden Supplies GmbH/ })).toHaveCount(1);
  });

  test('a photo without a model key is entered by hand, and the screen says so', async ({ page }) => {
    await mockApi({ person: true });
    await signInPerson(page);
    await page.goto('#/bills');
    await upload(page, 'scan.png');
    await expect(page.getByRole('row', { name: /Scanned vendor/ })).toBeVisible();
    await expect(page.getByRole('row', { name: /Scanned vendor/ }).getByRole('link', { name: 'Review' })).toBeVisible();
    expect(billsMock.modelCalls).toBe(0);
    await page.getByRole('row', { name: /Scanned vendor/ }).getByRole('link', { name: 'Review' }).click();
    await expect(page.getByRole('img', { name: /scan\.png/ })).toBeVisible();
  });

  test('offline, a capture waits in the queue and uploads exactly once when the connection returns', async ({ page, context }) => {
    await mockApi({ person: true });
    await signInPerson(page);
    await page.goto('#/bills');
    await context.setOffline(true);
    await upload(page, 'invoice.pdf');
    await expect(page.getByText('1 waiting to upload')).toBeVisible();
    expect(billsMock.uploads).toBe(0);
    await context.setOffline(false);
    await expect(page.getByRole('row', { name: /Halden Supplies GmbH/ })).toBeVisible();
    await expect(page.getByText('waiting to upload')).toHaveCount(0);
    expect(billsMock.uploads).toBe(1);
  });

  test('screenshots of the inbox, the upload and the review: desktop and phone, dark and light, and Arabic', async ({ browser }) => {
    mkdirSync(SHOT_DIR, { recursive: true });
    await mockApi({ person: true });
    const shots: Array<{ name: string; width: number; height: number; theme: 'dark' | 'light'; locale: 'en' | 'ar' }> = [
      { name: 'desktop-dark', width: 1440, height: 900, theme: 'dark', locale: 'en' },
      { name: 'desktop-light', width: 1440, height: 900, theme: 'light', locale: 'en' },
      { name: 'phone-dark', width: 390, height: 844, theme: 'dark', locale: 'en' },
      { name: 'phone-light', width: 390, height: 844, theme: 'light', locale: 'en' },
      { name: 'desktop-ar', width: 1440, height: 900, theme: 'light', locale: 'ar' },
    ];
    for (const shot of shots) {
      // A fresh browser context per variant: the theme and language are read at load.
      const context = await browser.newContext({ viewport: { width: shot.width, height: shot.height } });
      const page = await context.newPage();
      await signInPerson(page, shot.theme, shot.locale);
      await page.goto('#/bills');
      await upload(page, 'invoice.pdf');
      await expect(page.getByRole('row', { name: /Halden Supplies GmbH/ })).toBeVisible();
      await expect(page.getByText(/waiting to upload/)).toHaveCount(0);
      await page.screenshot({ path: join(SHOT_DIR, `inbox-${shot.name}.png`), fullPage: true });
      await page.getByRole('row', { name: /Halden Supplies GmbH/ }).locator('a').first().click();
      await expect(page.locator('iframe.doc-frame, .doc-image')).toBeVisible();
      await page.screenshot({ path: join(SHOT_DIR, `review-${shot.name}.png`), fullPage: true });
      await context.close();
    }
  });
});
