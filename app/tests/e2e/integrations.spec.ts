/**
 * Settings → Integrations and Overview's Costs by venture (issue #142), against
 * the fake API: the gallery and its coming-soon list, a token that is written
 * once and never shown, the mapping and its needs-mapping queue, a sync, an
 * agent key that reads but cannot connect, and the costs chart and its toggle.
 */
import { expect, test } from '@playwright/test';
import { connectState } from './mockConnect';
import { mockApi, signIn, signInPerson, stopMockApi } from './mockApi';

test.afterAll(stopMockApi);

test('the gallery offers the three sources, and coming soon is text only', async ({ page }) => {
  await mockApi({ seed: true, units: true, connect: true, person: true });
  await signInPerson(page);
  await page.goto('/#/settings/integrations');
  const gallery = page.locator('.gallery');
  await expect(gallery).toBeVisible();
  for (const name of ['Cloudflare', 'Anthropic', 'PlanetScale']) {
    await expect(gallery.getByRole('img', { name })).toBeVisible();
  }
  const soon = page.locator('.coming-soon');
  await expect(soon.getByRole('heading', { name: 'Coming soon' })).toBeVisible();
  await expect(soon.locator('button, a')).toHaveCount(0);
  await expect(soon.getByText('OpenAI')).toBeVisible();
});

test('a token is sent once, the field clears, and nothing shows it again', async ({ page }) => {
  const token = 'cfat_CLIENT_TEST_TOKEN_0001';
  await mockApi({ seed: true, units: true, person: true });
  await signInPerson(page);
  await page.goto('/#/settings/integrations');
  await page.getByRole('button', { name: /^Connect/ }).first().click();
  const form = page.locator('.connect-form');
  await form.getByLabel('Name').fill('Cloudflare (test)');
  await form.getByLabel('Cloudflare account ID').fill('0123456789abcdef0123456789abcdef');
  await form.getByLabel('Expense account').fill('6100');
  const tokenField = form.getByLabel('Token');
  await expect(tokenField).toHaveAttribute('type', 'password');
  await tokenField.fill(token);
  await form.getByRole('button', { name: 'Save connection' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Cloudflare (test) is connected' })).toBeVisible();
  expect(connectState.tokensReceived).toEqual([token]);
  await expect(page.locator('body')).not.toContainText(token);
  expect(await page.content()).not.toContain(token);
});

test('an unmapped resource is listed with a suggestion, and mapping it saves the rule', async ({ page }) => {
  await mockApi({ seed: true, units: true, connect: true, person: true });
  await signInPerson(page);
  await page.goto('/#/settings/integrations');
  await page.getByRole('button', { name: /Manage/ }).first().click();
  const queue = page.locator('.queue');
  await expect(queue.getByText('Looks like SQZ')).toBeVisible();
  await queue.getByRole('button', { name: 'Map to SQZ' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Mapping saved' })).toBeVisible();
  const saved = connectState.writes.find((write) => write.path.startsWith('/v1/allocation-rules/'));
  expect(saved).toBeTruthy();
  expect(JSON.stringify(saved?.body)).toContain('sqeezin-worker');
  await expect(queue.getByText('tmp-preview-worker')).toBeVisible();
});

test('a sync runs and its result is in the history', async ({ page }) => {
  await mockApi({ seed: true, units: true, connect: true, person: true });
  await signInPerson(page);
  await page.goto('/#/settings/integrations');
  await page.getByRole('button', { name: /Manage/ }).first().click();
  await page.getByRole('button', { name: 'Sync now' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Sync finished' })).toBeVisible();
  expect(connectState.writes.some((write) => write.path.endsWith('/sync'))).toBe(true);
  await expect(page.getByRole('table', { name: 'Sync history' })).toContainText('Succeeded');
  await expect(page.getByRole('table', { name: 'Sync history' })).toContainText('refused the token');
});

test('an agent key reads the integrations but cannot connect or sync', async ({ page }) => {
  await mockApi({ seed: true, units: true, connect: true });
  await signIn(page);
  await page.goto('/#/settings/integrations');
  await expect(page.getByText('Connections are set up and synced by an owner or admin')).toBeVisible();
  await expect(page.getByRole('button', { name: /^Connect/ })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Sync now' })).toHaveCount(0);
});

test('Overview shows costs by venture, and the toggle stacks by source', async ({ page }) => {
  await mockApi({ seed: true, units: true, connect: true, person: true });
  await signInPerson(page);
  await page.goto('/#/overview');
  const panel = page.locator('.costs');
  await expect(panel.locator('.chart-title', { hasText: 'Costs by venture' })).toBeVisible();
  await expect(panel.locator('svg rect').first()).toBeVisible();
  await expect(panel.getByRole('group')).toBeVisible();
  await expect(panel.getByRole('button', { name: 'By venture' })).toHaveAttribute('aria-pressed', 'true');
  await panel.getByRole('button', { name: 'By source' }).click();
  await expect(panel.getByRole('button', { name: 'By source' })).toHaveAttribute('aria-pressed', 'true');
  await expect(panel.locator('.chart-legend')).toContainText('Cloudflare');
  await expect(panel.locator('.chart-legend')).toContainText('PlanetScale');
  await expect(page.getByText('Also in USD, not charted')).toBeVisible();
});
