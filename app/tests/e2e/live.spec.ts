import { expect, test } from '@playwright/test';

// Runs against a real `ledgers-server` started with LEDGERS_DEV=1 on
// 127.0.0.1:3000. The key is the dev key the server printed (LIVE_API_KEY);
// nothing here starts the fake API.
const API_KEY = process.env.LIVE_API_KEY ?? '';
const API_ORIGIN = 'http://127.0.0.1:3000';

test.describe('live dev server: connect, post, reverse', () => {
  test.skip(!API_KEY, 'set LIVE_API_KEY to the dev key the server printed');

  test('connect → overview → new entry → journal → reverse', async ({ page }) => {
    await page.goto('/');
    await expect(page.getByRole('heading', { name: 'ledgers.sh' })).toBeVisible();
    await page.getByRole('button', { name: 'Advanced: connect with an agent key' }).click();
    await expect(page.getByLabel('Server URL')).toHaveValue(API_ORIGIN);
    await page.getByLabel('API key').fill(API_KEY);
    await page.getByRole('button', { name: 'Test connection' }).click();
    await expect(page.getByText(/Connected\. /)).toBeVisible();
    await page.getByRole('button', { name: 'Connect', exact: true }).click();

    await expect(page.getByRole('heading', { name: 'Overview' })).toBeVisible();
    await page.getByRole('link', { name: 'New entry' }).first().click();
    await expect(page.getByRole('heading', { name: 'New entry' })).toBeVisible();
    await page.getByLabel('Description').fill('Consulting invoice');
    await page.getByLabel('Line 1 account').selectOption('5010');
    await page.getByLabel('Line 1 amount').fill('850.00');
    await page.getByLabel('Line 2 account').selectOption('4010');
    await page.getByLabel('Line 2 side').selectOption('credit');
    await page.getByLabel('Line 2 amount').fill('850.00');
    await expect(page.getByText('Balanced', { exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Post entry' }).click();

    await expect(page.getByRole('heading', { name: 'Journal' })).toBeVisible();
    await expect(page.getByRole('cell', { name: 'Consulting invoice' })).toBeVisible();
    await page.getByRole('link', { name: /FY\d{4}-0001/ }).click();
    await page.getByRole('button', { name: 'Reverse entry' }).click();
    const dialog = page.getByRole('dialog');
    await expect(dialog.getByText(/new reversal entry/)).toBeVisible();
    await dialog.getByRole('button', { name: 'Post reversal' }).click();
    await expect(page.getByText(/Reversal posted as/)).toBeVisible();
    // The ledger describes a reversal as "Reversal of entry #<number>".
    await expect(page.getByRole("cell", { name: /Reversal of entry #1/ })).toBeVisible();
  });
});
