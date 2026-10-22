import { expect, test } from '@playwright/test';
import { mockApi, stopMockApi } from './mockApi';

test.describe('approvals inbox (issue #33)', () => {
  test.afterAll(stopMockApi);

  test('the inbox loads with the pending badge and the rows an agent asked for', async ({ page }) => {
    await mockApi({ person: true });
    await page.goto('/#/approvals');

    await expect(page.getByRole('heading', { name: 'Approvals' })).toBeVisible();
    // Five pending requests in the fixtures; the badge in the sidebar carries the count.
    await expect(page.locator('aside.sidebar .nav-count')).toHaveText('5');
    await expect(page.getByRole('link', { name: 'Halden Works GmbH' })).toBeVisible();
    await expect(page.getByRole('cell', { name: 'key_agent_ledger_01' }).first()).toBeVisible();
    await expect(page.getByRole('cell', { name: 'High risk' })).toBeVisible();
    await expect(page.getByRole('cell', { name: '125.00 EUR' })).toBeVisible();
  });

  test('the overview shows what is waiting, with a link to the inbox', async ({ page }) => {
    await mockApi({ person: true });
    await page.goto('/#/overview');
    await expect(page.getByRole('heading', { name: 'Waiting for you' })).toBeVisible();
    await expect(page.getByRole('link', { name: 'Review approvals' })).toHaveAttribute('href', '#/approvals');
    await expect(page.getByRole('link', { name: /Nordvik Office AS/ })).toBeVisible();
  });

  test('a deep link opens the detail with the entry preview and the requesting agent', async ({ page }) => {
    await mockApi({ person: true });
    await page.goto('/#/approvals/apr-low-1');

    await expect(page.getByRole('heading', { name: 'Approval request' })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'Entry preview' })).toBeVisible();
    await expect(page.getByRole('cell', { name: '6000', exact: true })).toBeVisible();
    await expect(page.getByRole('cell', { name: '2000', exact: true })).toBeVisible();
    await expect(page.getByText('exceeds per-entry limit')).toBeVisible();
    await expect(page.getByText('key_agent_ledger_01').first()).toBeVisible();
    await expect(page.getByText('Halden Works GmbH').first()).toBeVisible();
  });

  test('approve shows the result and posts the decision', async ({ page }) => {
    const api = await mockApi({ person: true });
    await page.goto('/#/approvals/apr-low-1');

    await page.getByRole('button', { name: 'Approve', exact: true }).click();
    await expect(page.getByText('Approved.', { exact: true })).toBeVisible();
    await expect(page.locator('.banner').getByRole('link', { name: 'entry-apr-low-1' })).toBeVisible();
    expect(api.decisions).toEqual([{ id: 'apr-low-1', decision: 'approve' }]);
  });

  test('reject sends the reason and shows the decision', async ({ page }) => {
    const api = await mockApi({ person: true });
    await page.goto('/#/approvals/apr-low-2');

    await page.getByLabel('Reason (optional)').fill('Duplicate of invoice 2026-118');
    await page.getByRole('button', { name: 'Reject', exact: true }).click();
    await expect(page.getByText('Rejected. Nothing was posted.')).toBeVisible();
    expect(api.decisions).toEqual([{ id: 'apr-low-2', decision: 'reject', reason: 'Duplicate of invoice 2026-118' }]);
  });

  test('bulk select refuses a high-risk row and a mixed-currency selection', async ({ page }) => {
    const api = await mockApi({ person: true });
    await page.goto('/#/approvals');

    const highRisk = page.getByRole('checkbox', { name: 'Select Kestrel Logistics for bulk approval' });
    await expect(highRisk).toBeDisabled();

    await page.getByRole('checkbox', { name: 'Select Halden Works GmbH for bulk approval' }).check();
    await expect(page.getByRole('button', { name: 'Approve selected (1)' })).toBeEnabled();

    await page.getByRole('checkbox', { name: 'Select Bayview Supplies for bulk approval' }).check();
    await expect(page.getByRole('button', { name: 'Approve selected (2)' })).toBeDisabled();
    await expect(page.getByText('The selection mixes EUR and USD. Select one currency at a time.')).toBeVisible();

    await page.getByRole('checkbox', { name: 'Select Bayview Supplies for bulk approval' }).uncheck();
    await page.getByRole('checkbox', { name: 'Select Nordvik Office AS for bulk approval' }).check();
    await expect(page.getByRole('button', { name: 'Approve selected (2)' })).toBeEnabled();
    expect(api.bulk).toEqual([]);
  });

  test('bulk approve asks first, then approves the low-risk rows together', async ({ page }) => {
    const api = await mockApi({ person: true });
    await page.goto('/#/approvals');

    await page.getByRole('checkbox', { name: 'Select Halden Works GmbH for bulk approval' }).check();
    await page.getByRole('checkbox', { name: 'Select Nordvik Office AS for bulk approval' }).check();
    await page.getByRole('button', { name: 'Approve selected (2)' }).click();

    const dialog = page.getByRole('dialog');
    await expect(dialog.getByText('Approve 2 requests?')).toBeVisible();
    await dialog.getByRole('button', { name: 'Approve 2 requests' }).click();
    await expect(page.locator('.notice').getByText('Approved 2 requests.')).toBeVisible();
    expect(api.bulk).toEqual([['apr-low-1', 'apr-low-2']]);
  });

  test('history lists decided requests with their audit reference', async ({ page }) => {
    await mockApi({ person: true });
    await page.goto('/#/approvals');

    await page.getByRole('tab', { name: 'History' }).click();
    await expect(page.getByRole('tab', { name: 'History' })).toHaveAttribute('aria-selected', 'true');
    await expect(page.getByRole('cell', { name: 'audit_01HZKQ7M' })).toBeVisible();
    await expect(page.getByRole('cell', { name: 'person_01' }).first()).toBeVisible();
    await expect(page.getByRole('cell', { name: 'Rejected', exact: true })).toBeVisible();
    // The pending rows are gone once the history loads (no stale rows from the other tab).
    await expect(page.getByRole('link', { name: 'Halden Works GmbH' })).toHaveCount(0);
  });

  test('reauth-required sends the browser to sign in again, returning to this approval', async ({ page }) => {
    const api = await mockApi({ person: true });
    const signIns: URL[] = [];
    await page.route('**/v1/auth/login**', async (route) => {
      signIns.push(new URL(route.request().url()));
      await route.fulfill({ status: 200, contentType: 'text/html', body: '<p>Sign in</p>' });
    });
    await page.goto('/#/approvals/apr-high-1');

    await page.getByRole('button', { name: 'Approve', exact: true }).click();
    await expect.poll(() => signIns.length).toBe(1);
    const signIn = signIns[0];
    expect(signIn.pathname).toBe('/v1/auth/login');
    expect(signIn.searchParams.get('prompt')).toBe('login');
    expect(signIn.searchParams.get('return_to')).toBe('/approvals/apr-high-1');
    expect(api.decisions).toEqual([]);
  });

  test('an expired request says so and shows its current state', async ({ page }) => {
    await mockApi({ person: true });
    await page.goto('/#/approvals/apr-expired-1');

    await page.getByRole('button', { name: 'Approve', exact: true }).click();
    await expect(page.getByText(/expired before it was approved/)).toBeVisible();
    await expect(page.getByRole('button', { name: 'Approve', exact: true })).toHaveCount(0);
    await expect(page.getByText('Expired', { exact: true }).first()).toBeVisible();
  });
});
