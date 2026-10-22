/**
 * Business units in the app, against the fake API: the top-bar switcher drives
 * every request and the URL (`?unit=`), the Overview's by-unit comparison, the
 * units screen (add, rename, archive), the recharge dialog's checks, the
 * required unit on an entry line, and an axe pass on each.
 */
import AxeBuilder from '@axe-core/playwright';
import { expect, test, type Page } from '@playwright/test';
import { choose } from './select';
import { mockApi, signIn, signInPerson, stopMockApi } from './mockApi';
import { unitState } from './mockUnits';

test.afterAll(stopMockApi);

test.beforeEach(async () => {
  await mockApi({ seed: true, units: true, person: true });
});

async function openUnits(page: Page, theme: 'dark' | 'light' = 'dark') {
  await signInPerson(page, theme);
  await page.goto('/#/overview');
  await expect(page.getByRole('combobox', { name: 'Business unit' })).toBeVisible();
}

test('the switcher writes the unit to the URL and every report request carries it', async ({ page }) => {
  await openUnits(page);
  await choose(page, 'Business unit', 'LDG');
  await expect(page).toHaveURL(/#\/overview\?unit=LDG$/);
  const scoped = page.waitForRequest((request) => request.url().includes('/v1/reports/trial-balance') && request.url().includes('unit=LDG'));
  await page.getByRole('navigation', { name: 'Sections' }).first().getByRole('link', { name: 'Reports' }).click();
  await expect(page).toHaveURL(/#\/reports\?unit=LDG$/);
  await scoped;
  // Back to the whole company: the parameter is removed and the next request has none.
  await choose(page, 'Business unit', 'All units (consolidated)');
  await expect(page).toHaveURL(/#\/reports$/);
  const consolidated = page.waitForRequest((request) => request.url().includes('/v1/reports/trial-balance') && !request.url().includes('unit='));
  await page.reload();
  await consolidated;
});

test('the unit in the URL is read back into the switcher and survives a reload', async ({ page }) => {
  await openUnits(page);
  await page.goto('/#/reports?unit=SQZ&period=2026-10');
  await expect(page.getByRole('combobox', { name: 'Business unit' })).toContainText('SQZ');
  await page.reload();
  await expect(page.getByRole('combobox', { name: 'Business unit' })).toContainText('SQZ');
  // A code that is not an active unit falls back to the whole company rather than sending it.
  await page.goto('/#/reports?unit=NOPE');
  await expect(page.getByRole('combobox', { name: 'Business unit' })).toContainText('All units');
});

test('the switcher is hidden when the company has no units', async ({ page }) => {
  await mockApi({ seed: true, person: true });
  await signInPerson(page);
  await page.goto('/#/overview');
  await expect(page.locator('.kpis')).toBeVisible();
  await expect(page.getByRole('combobox', { name: 'Business unit' })).toHaveCount(0);
});

test('the overview compares the units side by side and a card switches the scope', async ({ page }) => {
  await openUnits(page);
  const section = page.locator('.units-summary');
  await expect(section.getByRole('heading', { name: 'By business unit' })).toBeVisible();
  await expect(section.locator('.unit-card')).toHaveCount(3);
  await expect(section).toContainText('Recharges between units are removed from company totals');
  await expect(section.locator('.unit-card').first()).toContainText('FZ');
  await section.locator('.unit-card', { hasText: 'LDG' }).click();
  await expect(page).toHaveURL(/#\/overview\?unit=LDG$/);
  await expect(page.locator('.units-summary')).toHaveCount(0);
});

test('the units screen adds, renames and archives a unit, and the switcher follows', async ({ page }) => {
  await openUnits(page);
  await page.goto('/#/accounts/units');
  const table = page.getByRole('table', { name: 'Business units of this company' });
  await expect(table.getByRole('row')).toHaveCount(4);

  await page.getByRole('button', { name: 'Add unit' }).click();
  const dialog = page.getByRole('dialog', { name: 'Add a business unit' });
  await dialog.getByLabel('Code').fill('ab');
  await dialog.getByLabel('Name').fill('Aurora');
  await dialog.getByRole('button', { name: 'Add unit' }).click();
  await expect(page.getByText('Unit AB added')).toBeVisible();
  await expect(table.getByRole('row')).toHaveCount(5);
  expect(unitState.units.some((unit) => unit.code === 'AB' && unit.name === 'Aurora')).toBe(true);

  // Renaming keeps the code and changes the name.
  await table.getByRole('row', { name: /AB/ }).getByRole('button', { name: 'Edit' }).click();
  const edit = page.getByRole('dialog', { name: 'Edit AB' });
  await edit.getByLabel('Name').fill('Aurora Labs');
  await edit.getByRole('button', { name: 'Save changes' }).click();
  await expect(page.getByText('Unit saved')).toBeVisible();
  await expect(table).toContainText('Aurora Labs');

  // Archive asks first, and the archived unit leaves the switcher.
  await table.getByRole('row', { name: /AB/ }).getByRole('button', { name: 'Archive' }).click();
  const confirm = page.getByRole('dialog', { name: 'Archive AB?' });
  await expect(confirm).toBeVisible();
  await confirm.getByRole('button', { name: 'Archive unit' }).click();
  await expect(page.getByText('Unit AB archived')).toBeVisible();
  await expect(table.getByRole('row', { name: /AB/ })).toContainText('Archived');
  await page.goto('/#/overview');
  await page.getByRole('combobox', { name: 'Business unit' }).click();
  await expect(page.getByRole('option', { name: /AB/ })).toHaveCount(0);
});

test('a duplicate code is shown on the code field, from the server', async ({ page }) => {
  await openUnits(page);
  await page.goto('/#/accounts/units');
  await page.getByRole('button', { name: 'Add unit' }).click();
  const dialog = page.getByRole('dialog', { name: 'Add a business unit' });
  await dialog.getByLabel('Code').fill('LDG');
  await dialog.getByLabel('Name').fill('Second Ledgers');
  await dialog.getByRole('button', { name: 'Add unit' }).click();
  await expect(dialog.locator('#unit-code-error')).toHaveText('A unit with this code already exists.');
});

test('the recharge dialog checks the form before it posts, and shows what each unit receives', async ({ page }) => {
  await openUnits(page);
  await page.goto('/#/accounts/units');
  await page.getByRole('button', { name: 'Transfer between units' }).click();
  const dialog = page.getByRole('dialog', { name: 'Transfer between units' });
  await expect(dialog).toBeVisible();
  await dialog.getByRole('button', { name: 'Post transfer' }).click();
  await expect(dialog.getByText('Choose the unit that paid the cost.')).toBeVisible();
  await expect(dialog.getByText('Enter an amount greater than zero.')).toBeVisible();
  await expect(dialog.getByText('Describe the recharge.')).toBeVisible();
  expect(unitState.transfers).toHaveLength(0);

  await choose(page, 'From (paid the cost)', 'FZ');
  await choose(page, 'To (receives the cost)', 'FZ');
  await expect(dialog.getByText('Choose two different units.')).toBeVisible();
  await choose(page, 'To (receives the cost)', 'LDG');
  await dialog.getByLabel(/^Amount \(EUR\)$/).fill('250.00');
  await choose(page, 'Recovery account (income)', 'Sales');
  await expect(dialog.getByText('Choose an income account to recover the cost.')).toHaveCount(0);
  // The cost account lists expense accounts only: an income account is not offered.
  await page.getByRole('combobox', { name: 'Cost account (expense)' }).click();
  await expect(page.getByRole('option', { name: 'Sales' })).toHaveCount(0);
  await expect(page.getByRole('option', { name: 'Rent' })).toBeVisible();
  await page.keyboard.press('Escape');
  await choose(page, 'Cost account (expense)', 'Rent');
  await expect(dialog.locator('.transfer-preview')).toContainText('250.00');
  await dialog.getByLabel('Description').fill('Shared hosting, October');
  await dialog.getByRole('button', { name: 'Post transfer' }).click();
  await expect(page.getByText(/Transfer posted as entry FY2026-/)).toBeVisible();
  expect(unitState.transfers).toHaveLength(1);
  expect(unitState.transfers[0]).toMatchObject({ from_unit: 'FZ', to_unit: 'LDG', amount: { minor: '25000', currency: 'EUR' } });
});

test('an income or expense line needs a unit once the company has units', async ({ page }) => {
  await signIn(page, 'dark');
  await page.goto('/#/new');
  await expect(page.getByRole('combobox', { name: 'Business unit for line 1' })).toBeVisible();
  await choose(page, 'Line 1 account', /Sales/);
  await page.getByLabel('Description', { exact: true }).fill('Consulting income');
  await page.getByRole('textbox', { name: 'Line 1 amount' }).fill('100');
  await choose(page, 'Line 2 account', /Cash/);
  await page.getByRole('textbox', { name: 'Line 2 amount' }).fill('100');
  await page.getByRole('button', { name: 'Post entry' }).click();
  // The sales line has no unit: the form names it inline and does not send the entry.
  await expect(page.getByText('Choose a unit for this line.')).toBeVisible();
  expect(await page.getByRole('combobox', { name: 'Business unit for line 1' }).textContent()).toContain('Choose a unit');
  await choose(page, 'Business unit for line 1', 'LDG');
  await expect(page.getByText('Choose a unit for this line.')).toHaveCount(0);
});

test('an agent key is pinned to a unit at creation, and the pin reaches the server', async ({ page }) => {
  await signInPerson(page);
  await page.goto('/#/settings/agent-keys');
  await page.getByRole('button', { name: 'Create key', exact: true }).click();
  const form = page.getByRole('dialog', { name: 'Create an agent key' });
  await form.getByLabel('Name', { exact: true }).fill('Ledgers bot');
  await form.getByLabel('Most per entry in EUR').fill('1000.00');
  await form.getByLabel('Most per day in EUR').fill('5000.00');
  await form.getByRole('combobox', { name: 'Business unit' }).click();
  await page.getByRole('option', { name: 'LDG' }).click();
  const created = page.waitForRequest((request) => request.url().endsWith('/v1/keys') && request.method() === 'POST');
  await form.getByRole('button', { name: 'Create key', exact: true }).click();
  expect((await created).postDataJSON()).toMatchObject({ unit: 'LDG' });
});

test('a bank match to an income account needs a unit, and the unit reaches the post', async ({ page }) => {
  await mockApi({ seed: true, units: true, person: true, bank: true });
  await signInPerson(page);
  await page.goto('/#/bank');
  await page.getByRole('link', { name: /Girokonto Halden/ }).click();
  await page.locator('.bank-line').first().click();
  const panel = page.locator('#bank-reconcile');
  await panel.getByRole('radio', { name: 'Another account' }).check();
  await choose(page, 'Other account', 'Sales');
  const match = panel.getByRole('button', { name: 'Match', exact: true });
  await match.click();
  await expect(panel.getByText('Choose a unit for this line.')).toBeVisible();
  const posted = page.waitForRequest((request) => request.url().includes('/v1/bank/transactions/') && request.url().endsWith('/post'));
  await panel.getByRole('combobox', { name: 'Unit' }).click();
  await page.getByRole('option', { name: 'LDG' }).click();
  await match.click();
  expect((await posted).postDataJSON()).toMatchObject({ counterparty_account: '4000', unit: 'LDG' });
});

test('the units and the switcher pass axe', async ({ page }) => {
  await openUnits(page);
  for (const route of ['/#/overview?unit=LDG', '/#/accounts/units']) {
    await page.goto(route);
    await expect(page.getByRole('combobox', { name: 'Business unit' })).toBeVisible();
    const results = await new AxeBuilder({ page }).analyze();
    expect(results.violations, route).toEqual([]);
  }
});

test('the recharge dialog passes axe', async ({ page }) => {
  await openUnits(page);
  await page.goto('/#/accounts/units');
  await page.getByRole('button', { name: 'Transfer between units' }).click();
  await expect(page.getByRole('dialog', { name: 'Transfer between units' })).toBeVisible();
  const results = await new AxeBuilder({ page }).include('.transfer-dialog').analyze();
  expect(results.violations).toEqual([]);
});
