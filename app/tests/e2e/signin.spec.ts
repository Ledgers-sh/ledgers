import { expect, test } from '@playwright/test';
import { API_ORIGIN, mockApi, stopMockApi, WEB_ORIGIN } from './mockApi';

// People sign-in (issue #12) with the server's session endpoints mocked at the
// browser boundary. The real login round trip (passkey, issuer, callback) is
// the server's test suite; these specs cover what the app does with it.

const CORS = {
  'access-control-allow-origin': WEB_ORIGIN,
  'access-control-allow-credentials': 'true',
};

const OWNER = {
  personId: 'person-42',
  email: 'owner@halden.test',
  roles: ['owner'],
  csrfToken: 'csrf-token-for-the-spec',
  expiresAt: '2026-10-22T12:00:00Z',
};

test.describe('people sign-in', () => {
  test.afterAll(stopMockApi);

  test('signed out: a passkey button and no password field', async ({ page }) => {
    await mockApi();
    await page.goto('/');
    await expect(page.getByRole('link', { name: 'Sign in with passkey' })).toHaveAttribute(
      'href',
      `${API_ORIGIN}/v1/auth/login`,
    );
    await expect(page.locator('input[type="password"]')).toHaveCount(0);
    await expect(page.getByText(/passkey is a key kept by your device/i)).toBeHidden();
    await page.getByText('What is a passkey?').click();
    await expect(page.getByText(/proves it is you without a password/i)).toBeVisible();
  });

  test('a signed-in owner sees their email and can sign out', async ({ page }) => {
    await mockApi();
    let signedIn = true;
    await page.route('**/v1/auth/session', (route) =>
      signedIn
        ? route.fulfill({ status: 200, contentType: 'application/json', headers: CORS, body: JSON.stringify(OWNER) })
        : route.fulfill({ status: 401, contentType: 'application/problem+json', headers: CORS, body: '{"title":"Sign-in required"}' }),
    );
    let logouts = 0;
    await page.route('**/v1/auth/logout', (route) => {
      logouts += 1;
      signedIn = false;
      return route.fulfill({ status: 204, headers: CORS, body: '' });
    });

    await page.goto('/#/overview');
    const top = page.locator('.topbar .person');
    await expect(top).toContainText('owner@halden.test');
    await expect(top.locator('.avatar')).toHaveText('O');
    await expect(page.getByLabel('Server URL')).toHaveCount(0);

    await page.getByRole('button', { name: 'Sign out' }).click();
    await expect(page.getByRole('link', { name: 'Sign in with passkey' })).toBeVisible();
    expect(logouts).toBe(1);
  });

  test('a signed-in account without the owner role is told why, with 403', async ({ page }) => {
    await mockApi();
    await page.route('**/v1/auth/session', (route) =>
      route.fulfill({
        status: 403,
        contentType: 'application/problem+json',
        headers: CORS,
        body: JSON.stringify({
          title: 'This account cannot use this company',
          detail: 'guest@halden.test is signed in, but only the company owner or an admin can use this ledger.',
        }),
      }),
    );
    await page.goto('/');
    await expect(page.getByRole('heading', { name: 'This account cannot use this ledger' })).toBeVisible();
    await expect(page.getByText(/only the company owner or an admin/)).toBeVisible();
    await expect(page.getByRole('button', { name: 'Sign out' })).toBeVisible();
  });

  test('when sign-in is not configured the sign-in screen says so', async ({ page }) => {
    await mockApi();
    await page.route('**/v1/auth/session', (route) =>
      route.fulfill({
        status: 503,
        contentType: 'application/problem+json',
        headers: CORS,
        body: JSON.stringify({ title: 'Sign-in is not configured', detail: 'This server has no people sign-in issuer configured.' }),
      }),
    );
    await page.goto('/');
    await expect(page.getByRole('alert')).toContainText('no people sign-in issuer configured');
    await expect(page.getByRole('link', { name: 'Sign in with passkey' })).toBeVisible();
  });

  test('the advanced link opens the agent-key connect and back returns to sign-in', async ({ page }) => {
    await mockApi();
    await page.goto('/');
    await page.getByRole('button', { name: 'Advanced: connect with an agent key' }).click();
    await expect(page.getByLabel('API key')).toBeVisible();
    await page.getByRole('button', { name: 'Back to sign-in' }).click();
    await expect(page.getByRole('link', { name: 'Sign in with passkey' })).toBeVisible();
  });
});
