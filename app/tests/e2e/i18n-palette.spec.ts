/**
 * Language picker, Arabic layout, and the ⌘K command palette against the
 * registry's OpenAPI document (the fixture the Rust test keeps in step).
 */
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { expect, test, type Page } from '@playwright/test';
import { mockApi, signIn, stopMockApi, WEB_ORIGIN } from './mockApi';

const registry = JSON.parse(readFileSync(fileURLToPath(new URL('../fixtures/openapi.json', import.meta.url)), 'utf8')) as {
  paths: Record<string, Record<string, Record<string, string>>>;
};
const operations = Object.entries(registry.paths).flatMap(([path, item]) =>
  Object.entries(item).map(([method, op]) => ({ path, method: method.toUpperCase(), op })),
);

test.afterAll(stopMockApi);

test.describe('language picker', () => {
  test.beforeEach(async () => {
    await mockApi({ seed: true });
  });

  test('chooses a language by name, the page follows, and the choice is remembered', async ({ page }) => {
    await signIn(page, 'dark', 'en');
    await page.goto('/#/overview');
    await expect(page.getByRole('heading', { name: 'Overview', exact: true })).toBeVisible();

    await page.getByRole('button', { name: /^Language: English/ }).click();
    const search = page.getByRole('combobox', { name: 'Search languages' });
    await expect(search).toBeFocused();
    await search.fill('deu');
    await expect(page.getByRole('option', { name: 'Deutsch' })).toBeVisible();
    await search.press('Enter');

    await expect(page.getByRole('heading', { name: 'Übersicht', exact: true })).toBeVisible();
    await expect(page.locator('html')).toHaveAttribute('lang', 'de');
    await expect(page.locator('html')).toHaveAttribute('dir', 'ltr');

    // The choice is saved on this device; the test's own init script would reset it on a reload, so read it back.
    expect(await page.evaluate(() => window.localStorage.getItem('ledgers.locale'))).toBe('de');
  });

  test('Arabic sets right-to-left, keeps amounts left-to-right, and Escape closes without choosing', async ({ page }) => {
    await signIn(page, 'dark', 'en');
    await page.goto('/#/overview');
    await page.getByRole('button', { name: /^Language: English/ }).click();
    await page.getByRole('combobox', { name: 'Search languages' }).press('ArrowDown');
    await page.getByRole('combobox', { name: 'Search languages' }).press('Escape');
    await expect(page.getByRole('combobox', { name: 'Search languages' })).toHaveCount(0);
    await expect(page.locator('html')).toHaveAttribute('dir', 'ltr');

    await page.getByRole('button', { name: /^Language: English/ }).click();
    await page.getByRole('combobox', { name: 'Search languages' }).fill('ar');
    await page.getByRole('option', { name: 'العربية' }).click();

    await expect(page.locator('html')).toHaveAttribute('dir', 'rtl');
    await expect(page.locator('html')).toHaveAttribute('lang', 'ar');
    await expect(page.getByRole('heading', { name: 'نظرة عامة', exact: true })).toBeVisible();
    // Amounts are isolated left-to-right inside the right-to-left page, with Western digits.
    const amount = page.locator('.tile .num bdi').first();
    await expect(amount).toHaveAttribute('dir', 'ltr');
    await expect(amount).toHaveText(/^[0-9.,−]+$/);
  });

  test('a language with no match says so', async ({ page }) => {
    await signIn(page, 'dark', 'en');
    await page.goto('/#/overview');
    await page.getByRole('button', { name: /^Language: English/ }).click();
    await page.getByRole('combobox', { name: 'Search languages' }).fill('klingon');
    await expect(page.getByText('No language matches “klingon”.')).toBeVisible();
    await expect(page.getByRole('option')).toHaveCount(0);
  });
});

/** Waits for the app shell (the shortcut is listening only once it has mounted), then opens the palette. */
async function openPalette(page: Page) {
  await expect(page.locator('.topbar')).toBeVisible();
  await page.keyboard.press('Control+k');
  await expect(page.locator('dialog.palette')).toBeVisible();
}

test.describe('command palette', () => {
  test.beforeEach(async () => {
    await mockApi({ seed: true });
  });

  test('lists every OpenAPI command with its CLI, MCP tool and HTTP call', async ({ page }) => {
    await signIn(page, 'dark', 'en');
    await page.goto('/#/overview');
    await openPalette(page);
    const dialog = page.locator('dialog.palette');
    await expect(dialog).toBeVisible();
    await expect(page.locator('.palette-item')).toHaveCount(operations.length);

    for (const { path, method, op } of operations) {
      const name = op['x-ledgers-command'];
      const item = page.locator('.palette-item', { has: page.getByRole('heading', { name }) });
      await expect(item, name).toHaveCount(1);
      await expect(item.locator('.palette-value').nth(0), name).toHaveText(op['x-ledgers-cli']);
      await expect(item.locator('.palette-value').nth(1), name).toHaveText(op['x-ledgers-mcp']);
      await expect(item.locator('.palette-value').nth(2), name).toHaveText(`${method} ${path}`);
      await expect(item.getByRole('button', { name: /^Copy/ }), name).toHaveCount(3);
    }
  });

  test('copy buttons put the CLI line, the MCP name and the curl call on the clipboard', async ({ page, context }) => {
    await context.grantPermissions(['clipboard-read', 'clipboard-write']);
    await signIn(page, 'dark', 'en');
    await page.goto('/#/overview');
    await openPalette(page);
    const item = page.locator('.palette-item', { has: page.getByRole('heading', { name: 'entries.post' }) });

    await item.getByRole('button', { name: 'Copy CLI: entries.post' }).click();
    await expect(page.getByText('Copied CLI.')).toBeAttached();
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('ledgers entries post');

    await item.getByRole('button', { name: 'Copy MCP tool: entries.post' }).click();
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('entries_post');

    await item.getByRole('button', { name: 'Copy HTTP call: entries.post' }).click();
    expect(await page.evaluate(() => navigator.clipboard.readText())).toContain(`curl -X POST "$LEDGERS_URL/v1/entries"`);
  });

  test('a command with a screen opens it, and the palette closes', async ({ page }) => {
    await signIn(page, 'dark', 'en');
    await page.goto('/#/overview');
    await openPalette(page);
    await page.locator('.palette-item', { has: page.getByRole('heading', { name: 'entries.post' }) })
      .getByRole('button', { name: 'Open New entry' })
      .click();
    await expect(page).toHaveURL(/#\/new$/);
    await expect(page.locator('dialog.palette')).not.toBeVisible();
    await expect(page.getByRole('heading', { name: 'New entry', exact: true })).toBeVisible();
  });

  test('search narrows the list and Escape closes it', async ({ page }) => {
    await signIn(page, 'dark', 'en');
    await page.goto('/#/overview');
    await openPalette(page);
    await expect(page.getByRole('searchbox', { name: 'Search commands' })).toBeFocused();
    await page.getByRole('searchbox', { name: 'Search commands' }).fill('reports');
    await expect(page.locator('.palette-item')).toHaveCount(
      operations.filter((o) => String(o.op['x-ledgers-command']).startsWith('reports.')).length,
    );
    // Escape first clears the search text (native search field); a second Escape closes the palette.
    await page.keyboard.press('Escape');
    await expect(page.getByRole('searchbox', { name: 'Search commands' })).toHaveValue('');
    await page.keyboard.press('Escape');
    await expect(page.locator('dialog.palette')).not.toBeVisible();
  });

  test('a viewer without owner or admin role is told which commands they cannot run', async ({ page }) => {
    await page.route('**/v1/auth/session', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        headers: { 'access-control-allow-origin': WEB_ORIGIN, 'access-control-allow-credentials': 'true' },
        body: JSON.stringify({ personId: 'p1', email: 'viewer@halden.test', roles: ['viewer'], csrfToken: 't', expiresAt: null }),
      }),
    );
    await page.goto('/#/overview');
    await openPalette(page);
    const item = page.locator('.palette-item', { has: page.getByRole('heading', { name: 'entries.post' }) });
    await expect(item.getByText('Your role cannot run this.')).toBeVisible();
    await expect(item.getByRole('button', { name: 'Open New entry' })).toHaveCount(0);
    // Reads stay open to any signed-in person.
    const read = page.locator('.palette-item', { has: page.getByRole('heading', { name: 'accounts.list' }) });
    await expect(read.getByRole('button', { name: 'Open Accounts' })).toBeVisible();
  });
});
