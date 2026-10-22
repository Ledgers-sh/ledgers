import { expect, type Page } from '@playwright/test';

/**
 * Chooses an option from one of the app's Select comboboxes the way a person
 * does: open the trigger, then pick the option. `name` matches the trigger's
 * accessible name, `option` the option's text.
 */
export async function choose(page: Page, name: string | RegExp, option: string | RegExp) {
  const trigger = page.getByRole('combobox', { name });
  await trigger.click();
  await page.getByRole('option', { name: option }).click();
  await expect(trigger).toBeVisible();
}
