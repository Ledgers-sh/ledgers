import { expect, test } from '@playwright/test';
import { choose } from './select';
import { API_KEY, API_ORIGIN, mockApi, stopMockApi } from './mockApi';

test.describe('connect, post, reverse', () => {
  test.afterAll(stopMockApi);

  test('connect → overview → new entry → journal → reverse', async ({ page }) => {
    const api = await mockApi();
    await page.goto('/');

    // Signed out: the passkey sign-in is the front door. The agent-key connect is advanced.
    await expect(page.getByRole('heading', { name: 'ledgers.sh' })).toBeVisible();
    await expect(page.getByRole('link', { name: 'Sign in with passkey' })).toBeVisible();
    await page.getByRole('button', { name: 'Advanced: connect with an agent key' }).click();

    // Connect screen: default server URL, key kept in sessionStorage only.
    await expect(page.getByRole('heading', { name: 'Advanced: connect with an agent key' })).toBeVisible();
    await expect(page.getByLabel('Server URL')).toHaveValue(API_ORIGIN);
    await page.getByLabel('API key').fill(API_KEY);
    await page.getByRole('button', { name: 'Test connection' }).click();
    await expect(page.getByText(/Connected\. /)).toBeVisible();
    await page.getByRole('button', { name: 'Connect', exact: true }).click();

    const stored = await page.evaluate(() => ({
      session: window.sessionStorage.getItem('ledgers.apiKey'),
      local: Object.keys(window.localStorage).filter((k) => k.includes('apiKey')),
    }));
    expect(stored.session).toBe(API_KEY);
    expect(stored.local).toEqual([]);

    // Overview: an empty period tells the next step.
    await expect(page.getByRole('heading', { name: 'Overview' })).toBeVisible();
    await expect(page.getByRole('link', { name: 'Post your first entry' }).first()).toBeVisible();

    // New entry: live balance check, then post.
    await page.getByRole('link', { name: 'New entry' }).first().click();
    await expect(page.getByRole('heading', { name: 'New entry' })).toBeVisible();
    await page.getByLabel('Description').fill('Office rent October');
    await choose(page, 'Line 1 account', /6000/);
    await page.getByLabel('Line 1 amount').fill('850.00');
    await choose(page, 'Line 2 account', /1000/);
    await choose(page, 'Line 2 side', 'Credit');
    await page.getByLabel('Line 2 amount').fill('800.00');
    await expect(page.getByText('Debits exceed by 50.00 EUR')).toBeVisible();
    await page.getByRole('button', { name: 'Post entry' }).click();
    await expect(page.getByText('Make debits equal credits to post.')).toBeVisible();
    expect(api.posts).toHaveLength(0);

    await page.getByLabel('Line 2 amount').fill('850.00');
    await expect(page.getByText('Balanced', { exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Post entry' }).click();

    // Journal shows the new entry, with the person marker.
    await expect(page.getByRole('heading', { name: 'Journal' })).toBeVisible();
    await expect(page.getByRole('link', { name: /FY\d{4}-0001/ })).toBeVisible();
    await expect(page.getByRole('cell', { name: 'Office rent October' })).toBeVisible();
    expect(api.posts).toHaveLength(1);

    // Detail and reversal with confirm.
    await page.getByRole('link', { name: /FY\d{4}-0001/ }).click();
    await expect(page.getByRole('heading', { name: /Entry FY\d{4}-0001/ })).toBeVisible();
    await page.getByRole('button', { name: 'Reverse entry' }).click();
    const dialog = page.getByRole('dialog');
    await expect(dialog.getByText(/new reversal entry/)).toBeVisible();
    await dialog.getByRole('button', { name: 'Post reversal' }).click();
    await expect(page.getByText(/Reversal posted as/)).toBeVisible();
    await expect(page.getByRole('heading', { name: 'Journal' })).toBeVisible();
    expect(api.reversals).toHaveLength(1);
    await expect(page.getByRole('cell', { name: /Reversal of FY\d{4}-0001/ })).toBeVisible();
  });

  test('reports and accounts show posted activity', async ({ page }) => {
    await mockApi({ seed: true });
    await page.addInitScript(
      ({ key, origin }) => {
        window.sessionStorage.setItem('ledgers.apiKey', key);
        window.sessionStorage.setItem('ledgers.baseUrl', origin);
      },
      { key: API_KEY, origin: API_ORIGIN },
    );
    await page.goto('/#/reports');
    await expect(page.getByRole('tab', { name: 'Trial balance' })).toHaveAttribute('aria-selected', 'true');
    await expect(page.getByRole('cell', { name: '1200' })).toBeVisible();
    await page.getByRole('tab', { name: 'Balance sheet' }).click();
    await expect(page.getByRole('button', { name: 'Export CSV' })).toBeVisible();
    await page.goto('/#/accounts');
    await expect(page.getByRole('cell', { name: 'Accounts receivable' })).toBeVisible();
  });

});
