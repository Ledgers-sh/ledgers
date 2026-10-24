/**
 * Connected sources, the cloud connectors (issue #142, PR 1a): the AWS and
 * Vercel forms (least-privilege help, the credential fields sent as one
 * object, the optional tag and team), and the connection test, against the fake
 * API. No request leaves the browser.
 */
import { expect, test } from '@playwright/test';
import { connectState } from './mockConnect';
import { mockApi, signInPerson, stopMockApi } from './mockApi';

test.afterAll(stopMockApi);

test('the gallery offers AWS and Vercel with their marks, and neither is listed as coming soon', async ({ page }) => {
  await mockApi({ seed: true, units: true, person: true });
  await signInPerson(page);
  await page.goto('/#/settings/integrations');
  const gallery = page.locator('.gallery');
  for (const name of ['Amazon Web Services', 'Vercel']) {
    await expect(gallery.getByRole('img', { name })).toBeVisible();
  }
  await expect(page.locator('.coming-soon')).not.toContainText('Vercel');
  await expect(page.locator('.coming-soon')).not.toContainText('Amazon Web Services');
});

test('AWS names the IAM policy to create and sends both key fields as one credential', async ({ page }) => {
  await mockApi({ seed: true, units: true, person: true });
  await signInPerson(page);
  await page.goto('/#/settings/integrations');
  await page.locator('.gallery-card', { hasText: 'Amazon Web Services' }).getByRole('button', { name: /Connect/ }).click();
  const form = page.locator('.connect-form');
  await expect(form).toContainText('ce:GetCostAndUsage');
  await expect(form.getByLabel('Secret access key')).toHaveAttribute('type', 'password');
  await expect(form.getByLabel('Access key ID')).toHaveAttribute('type', 'text');
  await expect(form.getByLabel('Token')).toHaveCount(0);
  await form.getByLabel('Name').fill('AWS (test)');
  await form.getByLabel('Access key ID').fill('AKIDEXAMPLE');
  await form.getByLabel('Secret access key').fill('SECRET_EXAMPLE_0001');
  await form.getByLabel('Expense account').fill('6100');
  await form.getByRole('button', { name: 'Save connection' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'AWS (test) is connected' })).toBeVisible();
  expect(connectState.credentialsReceived).toEqual([{ access_key_id: 'AKIDEXAMPLE', secret_access_key: 'SECRET_EXAMPLE_0001' }]);
  await expect(page.locator('body')).not.toContainText('SECRET_EXAMPLE_0001');
  expect(await page.content()).not.toContain('SECRET_EXAMPLE_0001');
});

test('a Vercel team is optional, and a connection test reports the vendor answer', async ({ page }) => {
  await mockApi({ seed: true, units: true, person: true });
  await signInPerson(page);
  await page.goto('/#/settings/integrations');
  await page.locator('.gallery-card', { hasText: 'Vercel' }).getByRole('button', { name: /Connect/ }).click();
  const form = page.locator('.connect-form');
  await expect(form).toContainText('Billing');
  await form.getByLabel('Name').fill('Vercel (test)');
  await form.getByLabel('Team ID (optional)').fill('team_abc123');
  await form.getByLabel('Token').fill('vercel_TEST_TOKEN_0001');
  await form.getByLabel('Expense account').fill('6100');
  await form.getByRole('button', { name: 'Save connection' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Vercel (test) is connected' })).toBeVisible();
  expect(connectState.tokensReceived).toContain('vercel_TEST_TOKEN_0001');
  await page.getByRole('button', { name: /Manage/ }).first().click();
  await page.getByRole('button', { name: 'Test connection' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'The vendor answered' })).toBeVisible();
});

test('a failed connection test names the refusal and never shows the token', async ({ page }) => {
  await mockApi({ seed: true, units: true, person: true });
  await signInPerson(page);
  await page.goto('/#/settings/integrations');
  await page.locator('.gallery-card', { hasText: 'Vercel' }).getByRole('button', { name: /Connect/ }).click();
  const form = page.locator('.connect-form');
  await form.getByLabel('Name').fill('Broken Vercel');
  await form.getByLabel('Token').fill('vercel_TEST_TOKEN_0002');
  await form.getByLabel('Expense account').fill('6100');
  await form.getByRole('button', { name: 'Save connection' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Broken Vercel is connected' })).toBeVisible();
  await page.getByRole('button', { name: /Manage/ }).first().click();
  await page.getByRole('button', { name: 'Test connection' }).click();
  await expect(page.getByRole('alert').filter({ hasText: 'The test failed' })).toContainText('refused the token (403)');
  expect(await page.content()).not.toContain('vercel_TEST_TOKEN_0002');
});

test('Google Cloud takes the service account key as JSON and the export table, and sends the key once', async ({ page }) => {
  const key = '{"client_email":"ledgers@billing-proj-test.iam.gserviceaccount.com","private_key":"-----BEGIN PRIVATE KEY-----\\nTEST\\n-----END PRIVATE KEY-----\\n"}';
  await mockApi({ seed: true, units: true, person: true });
  await signInPerson(page);
  await page.goto('/#/settings/integrations');
  await page.locator('.gallery-card', { hasText: 'Google Cloud' }).getByRole('button', { name: /Connect/ }).click();
  const form = page.locator('.connect-form');
  await expect(form).toContainText('BigQuery Data Viewer');
  await expect(form.getByLabel('Service account key (JSON)').first()).toHaveJSProperty('tagName', 'TEXTAREA');
  await form.getByLabel('Name').fill('Google Cloud (test)');
  await form.getByLabel('Project that runs the query').fill('billing-proj-test');
  await form.getByLabel('Billing export dataset').fill('billing_export');
  await form.getByLabel('Billing export table').fill('gcp_billing_export_v1');
  await form.getByLabel('Service account key (JSON)').fill(key);
  await form.getByLabel('Expense account').fill('6100');
  await form.getByRole('button', { name: 'Save connection' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Google Cloud (test) is connected' })).toBeVisible();
  expect(connectState.credentialsReceived.at(-1)).toEqual({ service_account_json: key });
  expect(await page.content()).not.toContain('BEGIN PRIVATE KEY');
});

test('Azure takes the client secret, and names the tenant, client and subscription IDs', async ({ page }) => {
  await mockApi({ seed: true, units: true, person: true });
  await signInPerson(page);
  await page.goto('/#/settings/integrations');
  await page.locator('.gallery-card', { hasText: 'Microsoft Azure' }).getByRole('button', { name: /Connect/ }).click();
  const form = page.locator('.connect-form');
  await expect(form).toContainText('Cost Management Reader');
  await expect(form.getByLabel('Client secret')).toHaveAttribute('type', 'password');
  await form.getByLabel('Name').fill('Azure (test)');
  await form.getByLabel('Tenant ID').fill('11111111-2222-3333-4444-555555555555');
  await form.getByLabel('Client (app) ID').fill('66666666-7777-8888-9999-aaaaaaaaaaaa');
  await form.getByLabel('Subscription ID').fill('bbbbbbbb-cccc-dddd-eeee-ffffffffffff');
  await form.getByLabel('Client secret').fill('AZURE_SECRET_TEST_0001');
  await form.getByLabel('Expense account').fill('6100');
  await form.getByRole('button', { name: 'Save connection' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Azure (test) is connected' })).toBeVisible();
  expect(connectState.credentialsReceived.at(-1)).toEqual({ client_secret: 'AZURE_SECRET_TEST_0001' });
  await expect(page.locator('body')).not.toContainText('AZURE_SECRET_TEST_0001');
});

test('Supabase takes a personal access token and says that its prices are not booked', async ({ page }) => {
  await mockApi({ seed: true, units: true, person: true });
  await signInPerson(page);
  await page.goto('/#/settings/integrations');
  await page.locator('.gallery-card', { hasText: 'Supabase' }).getByRole('button', { name: /Connect/ }).click();
  const form = page.locator('.connect-form');
  await expect(form).toContainText('books no bills');
  await form.getByLabel('Name').fill('Supabase (test)');
  await form.getByLabel('Organization').fill('acme-labs');
  await form.getByLabel('Token').fill('sbp_TEST_TOKEN_0001');
  await form.getByLabel('Expense account').fill('6100');
  await form.getByRole('button', { name: 'Save connection' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Supabase (test) is connected' })).toBeVisible();
  expect(connectState.tokensReceived).toContain('sbp_TEST_TOKEN_0001');
});

test('OpenAI asks for an admin key and says that a project key cannot read costs', async ({ page }) => {
  await mockApi({ seed: true, units: true, person: true });
  await signInPerson(page);
  await page.goto('/#/settings/integrations');
  await page.locator('.gallery-card', { hasText: 'OpenAI' }).getByRole('button', { name: /Connect/ }).click();
  const form = page.locator('.connect-form');
  await expect(form).toContainText('A project key cannot read organization costs');
  await form.getByLabel('Name').fill('OpenAI (test)');
  await form.getByLabel('Token').fill('sk-admin-TEST_0001');
  await form.getByLabel('Expense account').fill('6200');
  await form.getByRole('button', { name: 'Save connection' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'OpenAI (test) is connected' })).toBeVisible();
  expect(connectState.tokensReceived).toContain('sk-admin-TEST_0001');
});

test('OpenRouter asks for a management key, and its help says that model calls are not possible with it', async ({ page }) => {
  await mockApi({ seed: true, units: true, person: true });
  await signInPerson(page);
  await page.goto('/#/settings/integrations');
  await page.locator('.gallery-card', { hasText: 'OpenRouter' }).getByRole('button', { name: /Connect/ }).click();
  const form = page.locator('.connect-form');
  await expect(form).toContainText('cannot make model calls');
  await form.getByLabel('Name').fill('OpenRouter (test)');
  await form.getByLabel('Token').fill('sk-or-mgmt-TEST_0001');
  await form.getByLabel('Expense account').fill('6200');
  await form.getByRole('button', { name: 'Save connection' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'OpenRouter (test) is connected' })).toBeVisible();
  expect(connectState.tokensReceived).toContain('sk-or-mgmt-TEST_0001');
});

test('Stripe takes a restricted key and its three accounts, and leaves an empty Connect account out', async ({ page }) => {
  await mockApi({ seed: true, units: true, person: true });
  await signInPerson(page);
  await page.goto('/#/settings/integrations');
  await page.locator('.gallery-card', { hasText: 'Stripe' }).getByRole('button', { name: /Connect/ }).click();
  const form = page.locator('.connect-form');
  await expect(form).toContainText('read only on Balance');
  await form.getByLabel('Name').fill('Stripe (test)');
  await form.getByLabel('Provider balance (clearing account)').fill('1035');
  await form.getByLabel('Revenue account', { exact: true }).fill('4010');
  await form.getByLabel('Suspense account for unknown categories').fill('2040');
  await form.getByLabel('Expense account').fill('5100');
  await form.getByLabel('Token').fill('rk_live_TEST_0001');
  await form.getByRole('button', { name: 'Save connection' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Stripe (test) is connected' })).toBeVisible();
  expect(connectState.tokensReceived).toContain('rk_live_TEST_0001');
  const saved = connectState.connections.at(-1);
  expect(saved?.config).toMatchObject({ clearing_account: '1035', revenue_account: '4010', suspense_account: '2040', expense_account: '5100' });
  expect(saved?.config).not.toHaveProperty('stripe_account');
});

test('Polar takes an organization access token and its clearing and revenue accounts, and an empty tax account is left out', async ({ page }) => {
  await mockApi({ seed: true, units: true, person: true });
  await signInPerson(page);
  await page.goto('/#/settings/integrations');
  await page.locator('.gallery-card', { hasText: 'Polar' }).getByRole('button', { name: /Connect/ }).click();
  const form = page.locator('.connect-form');
  await expect(form).toContainText('orders:read');
  await form.getByLabel('Name').fill('Polar (test)');
  await form.getByLabel('Provider balance (clearing account)').fill('1045');
  await form.getByLabel('Revenue account', { exact: true }).fill('4010');
  await form.getByLabel('Expense account').fill('5100');
  await form.getByLabel('Token').fill('polar_oat_TEST_0001');
  await form.getByRole('button', { name: 'Save connection' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Polar (test) is connected' })).toBeVisible();
  expect(connectState.tokensReceived).toContain('polar_oat_TEST_0001');
  const saved = connectState.connections.at(-1);
  expect(saved?.config).toMatchObject({ clearing_account: '1045', revenue_account: '4010', expense_account: '5100' });
  expect(saved?.config).not.toHaveProperty('tax_account');
});

test('Spaceship asks for an API key and secret, says which permission to create, and lists the domains due for renewal', async ({ page }) => {
  await mockApi({ seed: true, units: true, person: true });
  await signInPerson(page);
  await page.goto('/#/settings/integrations');
  await page.locator('.gallery-card', { hasText: 'Spaceship' }).getByRole('button', { name: /Connect/ }).click();
  const form = page.locator('.connect-form');
  await expect(form).toContainText('domains:read');
  await expect(form).toContainText('no billing');
  await expect(form.getByLabel('API key')).toHaveAttribute('type', 'password');
  await expect(form.getByLabel('API secret')).toHaveAttribute('type', 'password');
  await form.getByLabel('Name').fill('Spaceship (test)');
  await form.getByLabel('API key').fill('ss_key_TEST_0001');
  await form.getByLabel('API secret').fill('ss_secret_TEST_0001');
  await form.getByLabel('Expense account').fill('6100');
  await form.getByRole('button', { name: 'Save connection' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Spaceship (test) is connected' })).toBeVisible();
  await expect(page.getByRole('status').filter({ hasText: 'Run a sync to read its domains' })).toBeVisible();
  expect(connectState.credentialsReceived).toEqual([{ api_key: 'ss_key_TEST_0001', api_secret: 'ss_secret_TEST_0001' }]);
  expect(await page.content()).not.toContain('ss_secret_TEST_0001');
  await page.getByRole('button', { name: /Manage/ }).first().click();
  const renewals = page.locator('.renewals');
  await expect(renewals.getByRole('heading', { name: 'Upcoming renewals' })).toBeVisible();
  await expect(renewals).toContainText('squeez.in');
  await expect(renewals).toContainText('2026-11-20');
  await expect(renewals).toContainText('past expiry');
  await expect(renewals).not.toContainText('factory0');
});

test('Twilio takes an API key SID and secret with the account SID, and leaves an empty subaccount out', async ({ page }) => {
  await mockApi({ seed: true, units: true, person: true });
  await signInPerson(page);
  await page.goto('/#/settings/integrations');
  await page.locator('.gallery-card', { hasText: 'Twilio' }).getByRole('button', { name: /Connect/ }).click();
  const form = page.locator('.connect-form');
  await expect(form).toContainText('never the main auth token');
  await form.getByLabel('Name').fill('Twilio (test)');
  await form.getByLabel('Account SID (AC…)').fill('AC-test-account-0001');
  await form.getByLabel('API key SID (starts with SK)').fill('SK-test-key-sid-0001');
  await form.getByLabel('API key secret').fill('twilio_secret_TEST_0001');
  await form.getByLabel('Expense account').fill('6100');
  await form.getByRole('button', { name: 'Save connection' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Twilio (test) is connected' })).toBeVisible();
  expect(connectState.credentialsReceived.at(-1)).toEqual({ api_key_sid: 'SK-test-key-sid-0001', api_key_secret: 'twilio_secret_TEST_0001' });
  const saved = connectState.connections.at(-1);
  expect(saved?.config).toMatchObject({ account_sid: 'AC-test-account-0001', expense_account: '6100' });
  expect(saved?.config).not.toHaveProperty('subaccount_sid');
});
