/**
 * The journal: a page at a time with Load more, every row a real link, and a
 * click anywhere on a row (or Enter on its link) opening the entry's detail
 * with its lines. Uses an id with hyphens, like the demo book's, and an entry
 * posted without an external id, which the detail reaches through the listing.
 */
import { expect, test } from '@playwright/test';
import { mockApi, signIn, stopMockApi } from './mockApi';

test.describe('journal', () => {
  test.afterAll(stopMockApi);

  test('loads a page at a time, and Load more fetches the rest', async ({ page }) => {
    await mockApi({ extraEntries: 60 });
    await signIn(page);
    await page.goto('/#/journal');

    const rows = page.locator('tbody tr.journal-row');
    await expect(rows).toHaveCount(50);
    const more = page.getByRole('button', { name: 'Load more entries' });
    await expect(more).toBeVisible();
    await more.click();
    await expect(rows).toHaveCount(60);
    await expect(page.getByRole('button', { name: 'Load more entries' })).toHaveCount(0);
  });

  test('clicking a row opens its entry with the lines (hyphenated id)', async ({ page }) => {
    await mockApi({ extraEntries: 3 });
    await signIn(page);
    await page.goto('/#/journal');

    // The description cell is not a link: the whole row opens the entry.
    await page.getByRole('cell', { name: 'Bank charges and loan interest, Dezember' }).click();
    await expect(page).toHaveURL(/#\/journal\/halden-12-bank-charges-and-interest$/);
    await expect(page.getByRole('heading', { name: /FY\d{4}-0003/ })).toBeVisible();
    await expect(page.getByText('Bank charges and loan interest, Dezember').first()).toBeVisible();
    const lines = page.getByRole('table').last();
    await expect(lines.getByRole('cell', { name: '5100' })).toBeVisible();
    await expect(lines.getByRole('cell', { name: '1200' })).toBeVisible();
  });

  test('the row link is keyboard accessible', async ({ page }) => {
    await mockApi({ extraEntries: 3 });
    await signIn(page);
    await page.goto('/#/journal');

    const link = page.getByRole('link', { name: /FY\d{4}-0003/ }).first();
    await link.focus();
    await expect(link).toBeFocused();
    await page.keyboard.press('Enter');
    await expect(page).toHaveURL(/#\/journal\/halden-12-bank-charges-and-interest$/);
    await expect(page.getByRole('table').last().getByRole('cell', { name: '5100' })).toBeVisible();
  });

  test('an entry without an external id opens through the journal listing', async ({ page }) => {
    await mockApi({ extraEntries: 3 });
    await signIn(page);
    await page.goto('/#/journal');

    await page.getByRole('cell', { name: 'Generated entry 4' }).click();
    await expect(page).toHaveURL(/#\/journal\/demo-01M5HTFRTE2D7Y0M9YXBDZECM4$/);
    await expect(page.getByRole('table').last().getByRole('cell', { name: '5100' })).toBeVisible();
  });
});
