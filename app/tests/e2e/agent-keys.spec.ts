/**
 * Settings → Agent keys, end to end against the fake API: a signed-in person
 * creates a key (the token is shown once, and copied), sees only its prefix
 * afterwards, and revokes it after a confirm. An agent key gets the explanation
 * instead. Also writes screenshots of the screen into app/screenshots/.
 */
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test, type Page } from '@playwright/test';
import { mockApi, signIn, stopMockApi } from './mockApi';

const here = path.dirname(fileURLToPath(import.meta.url));
const outDir = path.resolve(here, '../../screenshots');

/** Opens the create dialog, fills it in, and returns the show-once dialog. */
async function createKey(page: Page, name: string, perEntry = '1000.00', perDay = '5000.00') {
  await page.getByRole('button', { name: 'Create key', exact: true }).click();
  const form = page.getByRole('dialog', { name: 'Create an agent key' });
  await form.getByLabel('Name', { exact: true }).fill(name);
  await form.getByLabel('Most per entry in EUR').fill(perEntry);
  await form.getByLabel('Most per day in EUR').fill(perDay);
  await form.getByRole('button', { name: 'Create key', exact: true }).click();
  const shown = page.getByRole('dialog', { name: 'Copy your new key' });
  await expect(shown).toBeVisible();
  const token = await shown.locator('input.secret').inputValue();
  return { shown, token };
}

/** The key's public prefix: everything before its 64-digit secret. */
const prefixOf = (token: string) => token.slice(0, token.length - 65);

test.describe('agent keys', () => {
  test.afterAll(stopMockApi);

  test('a person creates a key: the token is shown once and copied, and the list keeps only the prefix', async ({ page, context }) => {
    await context.grantPermissions(['clipboard-read', 'clipboard-write']);
    await mockApi({ seed: true, person: true });
    await page.goto('/#/settings/agent-keys');
    await expect(page.getByRole('heading', { name: 'Agent keys', exact: true })).toBeVisible();
    await expect(page.getByText('No agent keys yet')).toBeVisible();

    const { shown, token } = await createKey(page, 'Bank bot');
    await expect(shown.getByText("You won't see this again.")).toBeVisible();
    expect(token).toMatch(/^lk_live_[0-9a-f]{16}_[0-9a-f]{64}$/);
    await expect(shown.locator('pre').first()).toContainText(`LEDGERS_API_KEY=${token}`);
    await expect(shown.locator('pre').nth(1)).toContainText(`Bearer ${token}`);

    await shown.getByRole('button', { name: 'Copy key' }).click();
    await expect(shown.getByRole('button', { name: 'Copied', exact: true })).toBeVisible();
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(token);

    await shown.getByRole('button', { name: "I've copied the key" }).click();
    await expect(shown).toBeHidden();
    const row = page.getByRole('row', { name: /Bank bot/ });
    await expect(row).toContainText(prefixOf(token));
    await expect(row).toContainText('EUR 1000.00 / 5000.00');
    await expect(row).toContainText('Active');
    // Closed: the token is nowhere on the page.
    await expect(page.getByText(token)).toHaveCount(0);
  });

  test('revoking asks first, then the key is listed as revoked', async ({ page }) => {
    await mockApi({ seed: true, person: true });
    await page.goto('/#/settings/agent-keys');
    const { shown } = await createKey(page, 'Old feed');
    await shown.getByRole('button', { name: "I've copied the key" }).click();

    await page.getByRole('button', { name: 'Revoke Old feed' }).click();
    const confirm = page.getByRole('dialog', { name: 'Revoke Old feed?' });
    await expect(confirm).toBeVisible();
    await confirm.getByRole('button', { name: 'Cancel' }).click();
    await expect(confirm).toBeHidden();
    await expect(page.getByRole('row', { name: /Old feed/ })).toContainText('Active');

    await page.getByRole('button', { name: 'Revoke Old feed' }).click();
    await page.getByRole('dialog', { name: 'Revoke Old feed?' }).getByRole('button', { name: 'Revoke key' }).click();
    const row = page.getByRole('row', { name: /Old feed/ });
    await expect(row).toContainText('Revoked');
    await expect(page.getByRole('button', { name: 'Revoke Old feed' })).toHaveCount(0);
  });

  test('a name is required and nothing is sent until the form is valid', async ({ page }) => {
    await mockApi({ seed: true, person: true });
    await page.goto('/#/settings/agent-keys');
    await page.getByRole('button', { name: 'Create key', exact: true }).click();
    const form = page.getByRole('dialog', { name: 'Create an agent key' });
    await form.getByRole('button', { name: 'Create key', exact: true }).click();
    await expect(form.getByRole('alert')).toContainText('Give the key a name');
    await expect(page.getByRole('dialog', { name: 'Copy your new key' })).toBeHidden();
  });

  test('an agent key gets the explanation, not the key list', async ({ page }) => {
    await mockApi({ seed: true });
    await signIn(page);
    await page.goto('/#/settings/agent-keys');
    await expect(page.getByText('Sign in as a person to manage agent keys')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Create key', exact: true })).toHaveCount(0);
  });
});

for (const viewport of [
  { label: 'desktop', width: 1440, height: 900 },
  { label: 'phone', width: 390, height: 844 },
]) {
  for (const theme of ['dark', 'light'] as const) {
    test.describe(`screenshots ${viewport.label} ${theme}`, () => {
      test.afterAll(stopMockApi);
      test.use({ viewport: { width: viewport.width, height: viewport.height } });

      test('agent keys screen and dialogs', async ({ page }) => {
        await mockApi({ seed: true, person: true });
        await page.addInitScript((t) => window.localStorage.setItem('ledgers.theme', t), theme);
        await page.goto('/#/settings/agent-keys');
        await expect(page.getByRole('heading', { name: 'Agent keys', exact: true })).toBeVisible();
        await page.addStyleTag({ content: '.bottom-nav { position: static !important; margin-top: 24px; }' });
        await page.screenshot({ path: path.join(outDir, `${viewport.label}-${theme}-agent-keys-empty.png`), fullPage: true });

        const { shown } = await createKey(page, 'Bank bot');
        await page.screenshot({ path: path.join(outDir, `${viewport.label}-${theme}-agent-keys-show-once.png`), fullPage: true });
        await shown.getByRole('button', { name: "I've copied the key" }).click();
        await page.getByRole('button', { name: 'Revoke Bank bot' }).click();
        await page.getByRole('dialog', { name: 'Revoke Bank bot?' }).waitFor();
        await page.screenshot({ path: path.join(outDir, `${viewport.label}-${theme}-agent-keys-revoke.png`), fullPage: true });
        await page.getByRole('dialog', { name: 'Revoke Bank bot?' }).getByRole('button', { name: 'Cancel' }).click();
        await page.getByRole('button', { name: 'Create key', exact: true }).click();
        await page.getByRole('dialog', { name: 'Create an agent key' }).waitFor();
        await page.screenshot({ path: path.join(outDir, `${viewport.label}-${theme}-agent-keys-create.png`), fullPage: true });
      });
    });
  }
}
