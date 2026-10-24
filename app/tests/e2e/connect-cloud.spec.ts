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
