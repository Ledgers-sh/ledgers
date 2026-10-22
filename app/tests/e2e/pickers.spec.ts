/**
 * The designed pickers, by keyboard and pointer: the searchable account
 * combobox on the new-entry form, and the period popover (Escape, outside click,
 * focus trap). The language picker is covered in i18n-palette.spec.ts.
 */
import { expect, test } from '@playwright/test';
import { mockApi, signIn, stopMockApi } from './mockApi';

test.afterAll(stopMockApi);

test.beforeEach(async () => {
  await mockApi({ seed: true });
});

test('the account picker filters as you type, chooses with Enter, and shows the account number', async ({ page }) => {
  await signIn(page, 'dark', 'en');
  await page.goto('/#/new');
  const trigger = page.getByRole('combobox', { name: 'Line 1 account' });
  await trigger.click();
  const listbox = page.getByRole('listbox');
  const search = page.getByRole('combobox', { name: 'Search options' });
  await expect(search).toBeFocused();
  await search.fill('rent');
  await expect(page.getByRole('option')).toHaveCount(1);
  await expect(page.getByRole('option')).toContainText('6000');
  await search.press('Enter');
  await expect(listbox).toHaveCount(0);
  await expect(trigger).toBeFocused();
  await expect(trigger).toContainText('Rent');
});

test('the account picker says when nothing matches', async ({ page }) => {
  await signIn(page, 'dark', 'en');
  await page.goto('/#/new');
  await page.getByRole('combobox', { name: 'Line 1 account' }).click();
  await page.getByRole('combobox', { name: 'Search options' }).fill('zzzz');
  await expect(page.locator('.sel-none')).toHaveText('No match for “zzzz”.');
  await expect(page.getByRole('option')).toHaveCount(0);
});

test('a select closes on Escape with focus back on the trigger, and on an outside click without choosing', async ({ page }) => {
  await signIn(page, 'dark', 'en');
  await page.goto('/#/new');
  const trigger = page.getByRole('combobox', { name: 'Line 1 side' });
  await trigger.focus();
  await page.keyboard.press('ArrowDown');
  await expect(page.getByRole('listbox')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('listbox')).toHaveCount(0);
  await expect(trigger).toBeFocused();

  await trigger.click();
  await page.keyboard.press('ArrowDown');
  await page.getByRole('heading', { name: 'New entry' }).click();
  await expect(page.getByRole('listbox')).toHaveCount(0);
  await expect(trigger).toContainText('Debit');
});

test('arrow keys and Enter choose in a select without search', async ({ page }) => {
  await signIn(page, 'dark', 'en');
  await page.goto('/#/new');
  const trigger = page.getByRole('combobox', { name: 'Line 1 side' });
  await trigger.click();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  await expect(trigger).toContainText('Credit');
});

test('the period popover opens from the keyboard, moves through presets, and Escape returns focus to the trigger', async ({ page }) => {
  await signIn(page, 'dark', 'en');
  await page.goto('/#/overview');
  const trigger = page.locator('.pp-trigger');
  await trigger.focus();
  await page.keyboard.press('Enter');
  const dialog = page.getByRole('dialog');
  await expect(dialog).toBeVisible();
  await expect(page.getByRole('radio', { name: 'This year' })).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await expect(page.getByRole('radio', { name: 'Last 12 months' })).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await expect(page.getByRole('radio', { name: 'Custom' })).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(dialog).toHaveCount(0);
  await expect(trigger).toBeFocused();
});

test('the period popover closes on an outside click and keeps Tab inside while open', async ({ page }) => {
  await signIn(page, 'dark', 'en');
  await page.goto('/#/overview');
  await page.locator('.pp-trigger').click();
  const pop = page.locator('.pp-pop');
  await expect(pop).toBeVisible();
  for (let step = 0; step < 12; step += 1) {
    await page.keyboard.press('Tab');
    const inside = await pop.evaluate((el) => el.contains(document.activeElement));
    expect(inside, `focus left the popover after ${step + 1} tabs`).toBe(true);
  }
  // An empty spot outside the popover (the sidebar edge), so the click is not on any control.
  await page.mouse.click(4, 400);
  await expect(pop).toHaveCount(0);
});

test('a month in the custom grids sets the range, and the year steps', async ({ page }) => {
  await signIn(page, 'dark', 'en');
  await page.goto('/#/overview');
  await page.locator('.pp-trigger').click();
  const from = page.getByRole('group', { name: /^From / });
  await from.getByRole('button', { name: 'Mar' }).click();
  await expect(from.getByRole('button', { name: 'Mar' })).toHaveAttribute('aria-pressed', 'true');
  await expect(page.locator('.pp-trigger')).toHaveAttribute('aria-label', /Mar/);
  await page.getByRole('button', { name: 'Previous year' }).first().click();
  await expect(page.locator('.pp-year-value').first()).toHaveText(/From · \d{4}/);
});
