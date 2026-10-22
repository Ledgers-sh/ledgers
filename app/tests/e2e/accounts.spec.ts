/**
 * The chart of accounts: create (with inline validation), rename, archive and
 * restore, the balance rule for archiving, search and the status filter, and
 * an axe pass on the list, the form and an account page. A person session
 * writes; the fake API enforces the same rules the server does.
 */
import AxeBuilder from '@axe-core/playwright';
import { expect, test, type Page } from '@playwright/test';
import { mockApi, signInPerson, stopMockApi } from './mockApi';
import { choose } from './select';

test.afterAll(stopMockApi);

async function openAs(page: Page, hash: string, locale: 'en' | 'ar' = 'en') {
  await mockApi({ seed: true, person: true });
  await signInPerson(page, 'dark', locale);
  await page.goto(`/${hash}`);
}

test.describe('chart of accounts', () => {
  test('the list groups accounts by class, indents children and hides archived ones by default', async ({ page }) => {
    await openAs(page, '#/accounts');
    await expect(page.getByRole('heading', { name: 'Assets' })).toBeVisible();
    await expect(page.getByRole('link', { name: 'Office equipment' })).toBeVisible();
    await expect(page.getByRole('link', { name: 'Old telephone' })).toHaveCount(0);
    await page.getByRole('tab', { name: 'Archived' }).click();
    await expect(page.getByRole('link', { name: 'Old telephone' })).toBeVisible();
    await expect(page.getByRole('link', { name: 'Office equipment' })).toHaveCount(0);
  });

  test('search keeps the heading of a match in place', async ({ page }) => {
    await openAs(page, '#/accounts');
    await page.getByRole('searchbox', { name: 'Search accounts' }).fill('office');
    await expect(page.getByRole('link', { name: 'Office equipment' })).toBeVisible();
    await expect(page.getByRole('link', { name: 'Fixed assets' })).toBeVisible();
    await expect(page.getByRole('link', { name: 'Cash', exact: true })).toHaveCount(0);
  });

  test('create an account: inline checks, then the new account opens', async ({ page }) => {
    await openAs(page, '#/accounts/new');
    await page.getByRole('button', { name: 'Create account' }).click();
    await expect(page.locator('#acct-number-error')).toHaveText('Required.');
    await expect(page.locator('#acct-name-error')).toHaveText('Required.');

    await page.getByLabel('Account number').fill('1000');
    await expect(page.locator('#acct-number-error')).toHaveText('An account with this number already exists.');
    await page.getByLabel('Account number').fill('1600');
    await expect(page.locator('#acct-number-error')).toHaveCount(0);

    await choose(page, /Class/, 'Asset');
    await choose(page, /Parent account/, /1500/);
    await page.locator('#acct-name').fill('Vehicles');
    await page.getByLabel('Tags').fill('Vehicles');
    await expect(page.locator('#acct-tags-error')).toHaveText('Use lowercase letters, digits and single hyphens.');
    await page.getByLabel('Tags').fill('vehicles fleet');
    await page.getByRole('button', { name: 'Create account' }).click();

    await expect(page).toHaveURL(/#\/accounts\/1600$/);
    await expect(page.getByRole('heading', { level: 1, name: 'Vehicles' })).toBeVisible();
    await page.goto('/#/accounts');
    await expect(page.getByRole('link', { name: 'Vehicles' })).toBeVisible();
  });

  test('a parent of another class is refused inline', async ({ page }) => {
    await openAs(page, '#/accounts/new');
    // Pick an asset parent while the class is asset, then change the class: the parent stays chosen and is refused.
    await choose(page, /Class/, 'Asset');
    await choose(page, /Parent account/, /1500/);
    await choose(page, /Class/, 'Liability');
    await page.getByRole('button', { name: 'Create account' }).click();
    await expect(page.locator('#acct-parent-error')).toHaveText('The parent must be in the same class.');
  });

  test('rename an account: the custom name shows on the page and in the list', async ({ page }) => {
    await openAs(page, '#/accounts/1510');
    await expect(page.getByRole('heading', { level: 1, name: 'Office equipment' })).toBeVisible();
    await page.locator('#account-name').fill('Office furniture');
    await page.getByRole('button', { name: 'Save name' }).click();
    await expect(page.getByText('Saved.')).toBeVisible();
    await expect(page.getByRole('heading', { level: 1, name: 'Office furniture' })).toBeVisible();
    await page.goto('/#/accounts');
    await expect(page.getByRole('link', { name: 'Office furniture' })).toBeVisible();
  });

  test('archive is refused for an account with a balance, and allowed once it is zero', async ({ page }) => {
    await openAs(page, '#/accounts/1000');
    await page.getByRole('button', { name: 'Archive account' }).click();
    const dialog = page.getByRole('dialog', { name: 'Archive this account?' });
    await expect(dialog).toBeVisible();
    await expect(dialog.getByText('Archiving needs a zero balance.')).toBeVisible();
    await expect(dialog.getByRole('button', { name: 'Archive account' })).toBeDisabled();
    await page.keyboard.press('Escape');
    await expect(dialog).toBeHidden();

    await page.goto('/#/accounts/1510');
    await page.getByRole('button', { name: 'Archive account' }).click();
    await page.getByRole('dialog', { name: 'Archive this account?' }).getByRole('button', { name: 'Archive account' }).click();
    await expect(page.getByText('This account is archived. It takes no new postings.')).toBeVisible();
    await page.getByRole('button', { name: 'Restore account' }).click();
    await page.getByRole('dialog', { name: 'Restore this account?' }).getByRole('button', { name: 'Restore account' }).click();
    await expect(page.getByText('This account is archived.')).toHaveCount(0);
  });

  test('the account page lists recent entries and links to the journal entry', async ({ page }) => {
    await openAs(page, '#/accounts/1000');
    const link = page.getByRole('link', { name: /Entry / }).first();
    // Newest first: the rent entry (posted after the capital) is the account's latest line.
    await expect(link).toHaveAttribute('href', '#/journal/entry-seed-3');
  });

  test('the list and the form pass axe with no serious violations', async ({ page }) => {
    for (const hash of ['#/accounts', '#/accounts/new', '#/accounts/1000']) {
      await openAs(page, hash);
      await expect(page.locator('h1').first()).toBeVisible();
      const results = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa']).analyze();
      const blocking = results.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical');
      expect(blocking.map((v) => `${v.id}: ${v.help}`), hash).toEqual([]);
    }
  });
});
