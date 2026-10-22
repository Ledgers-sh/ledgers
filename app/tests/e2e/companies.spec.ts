/**
 * Companies in the app, against the fake API: the switcher lists the person's
 * companies with their roles and hides for one; choosing one writes it to the
 * URL, resets the unit scope and period, and reloads into it; every request
 * carries the company header; the create dialog makes a company and switches to
 * it, shows the server's refusal, and passes an axe check; and cached reads are
 * kept per company by the service worker.
 */
import AxeBuilder from '@axe-core/playwright';
import { expect, test, type Page } from '@playwright/test';
import { choose } from './select';
import { API_ORIGIN, mockApi, signInPerson, stopMockApi } from './mockApi';

test.afterAll(stopMockApi);

async function openAs(page: Page, path: string, companies: 'one' | 'many' | 'none' = 'many', units = false) {
  await mockApi({ seed: true, person: true, companies, units });
  await signInPerson(page);
  await page.goto(path);
  await expect(page.locator('.topbar')).toBeVisible();
}

test('the switcher hides for one company', async ({ page }) => {
  await openAs(page, '/#/overview', 'one');
  await expect(page.locator('.kpis')).toBeVisible();
  await expect(page.getByRole('combobox', { name: 'Company' })).toHaveCount(0);
});

test('the switcher lists both companies with their roles', async ({ page }) => {
  await openAs(page, '/#/overview', 'many');
  const trigger = page.getByRole('combobox', { name: 'Company' });
  await expect(trigger).toContainText('Halden Works GmbH');
  await trigger.click();
  await expect(page.getByRole('option', { name: /Halden Works GmbH/ })).toContainText('Owner');
  await expect(page.getByRole('option', { name: /Factory Zero/ })).toContainText('Viewer');
  await page.keyboard.press('Escape');
});

test('choosing a company writes it to the URL, resets the unit and period, and reloads into it', async ({ page }) => {
  await openAs(page, '/#/reports?unit=SQZ&period=2026-10', 'many', true);
  await expect(page.getByRole('combobox', { name: 'Business unit' })).toContainText('SQZ');
  await choose(page, 'Company', /Factory Zero/);
  await expect(page).toHaveURL(/\?company=FZDEMO02#\/overview$/);
  await expect(page.getByRole('combobox', { name: 'Company' })).toContainText('Factory Zero');
  await expect(page.getByRole('combobox', { name: 'Business unit' })).toContainText('All units');
  // The period is the default again: the saved range is gone until the person sets one.
  expect(await page.evaluate(() => window.sessionStorage.getItem('ledgers.range'))).toBeNull();
  // A reload keeps the company the URL names.
  await page.reload();
  await expect(page.getByRole('combobox', { name: 'Company' })).toContainText('Factory Zero');
});

test('every request after a switch carries the company header', async ({ page }) => {
  await openAs(page, '/?company=HALDEN01#/overview', 'many');
  const seen: { path: string; company: string | null }[] = [];
  page.on('request', (request) => {
    const url = new URL(request.url());
    if (url.origin === API_ORIGIN && url.pathname !== '/v1/openapi') {
      seen.push({ path: url.pathname, company: request.headers()['x-ledgers-company'] ?? null });
    }
  });
  await page.goto('/?company=FZDEMO02#/journal');
  await expect(page.locator('.kpis, table, .empty, h1').first()).toBeVisible();
  await expect.poll(() => seen.length).toBeGreaterThan(2);
  const wrong = seen.filter((call) => call.company !== 'FZDEMO02');
  expect(wrong).toEqual([]);
});

test('a person creates a company from Settings and the app switches to it', async ({ page }) => {
  await openAs(page, '/#/settings/company', 'many');
  await page.getByRole('button', { name: 'Create company' }).first().click();
  const dialog = page.getByRole('dialog', { name: 'Create company' });
  await expect(dialog).toBeVisible();
  await dialog.getByRole('textbox', { name: 'Company name' }).fill('Kestrel Labs Pte. Ltd.');
  await choose(page, 'Country pack', /Singapore/);
  await expect(dialog.getByRole('textbox', { name: 'Base currency' })).toHaveValue('SGD');

  const results = await new AxeBuilder({ page }).include('dialog[open]').analyze();
  expect(results.violations).toEqual([]);

  await dialog.getByRole('button', { name: 'Create company' }).click();
  await expect(page).toHaveURL(/\?company=NEW3X#\/overview$/);
  await expect(page.getByRole('combobox', { name: 'Company' })).toContainText('Kestrel Labs Pte. Ltd.');
});

test('a creation the server refuses shows its reason in the dialog', async ({ page }) => {
  // Two companies already: one creation is allowed, the next is refused by the fake (the limit is three).
  await openAs(page, '/#/settings/company', 'many');
  await page.getByRole('button', { name: 'Create company' }).first().click();
  const first = page.getByRole('dialog', { name: 'Create company' });
  await first.getByRole('textbox', { name: 'Company name' }).fill('Third company');
  await choose(page, 'Country pack', /Germany/);
  await first.getByRole('button', { name: 'Create company' }).click();
  await expect(page).toHaveURL(/\?company=NEW3X/);
  await expect(page.getByRole('combobox', { name: 'Company' })).toContainText('Third company');

  // Settings again, through the navigation, in the new company.
  await page.getByRole('navigation', { name: 'Sections' }).first().getByRole('link', { name: 'Settings' }).click();
  await expect(page.locator('.settings-nav')).toBeVisible();
  await page.getByRole('button', { name: 'Create company' }).first().click();
  const second = page.getByRole('dialog', { name: 'Create company' });
  await second.getByRole('textbox', { name: 'Company name' }).fill('Fourth company');
  await choose(page, 'Country pack', /Germany/);
  await second.getByRole('button', { name: 'Create company' }).click();
  await expect(second.getByRole('alert')).toContainText('already created 3 companies');
  await expect(page).not.toHaveURL(/NEW4X/);
});

test('the create dialog asks for a name before it sends anything', async ({ page }) => {
  await openAs(page, '/#/settings/company', 'many');
  await page.getByRole('button', { name: 'Create company' }).first().click();
  const dialog = page.getByRole('dialog', { name: 'Create company' });
  await dialog.getByRole('button', { name: 'Create company' }).click();
  await expect(dialog.locator('#create-company-name-error')).toContainText('Enter a company name.');
  await expect(page).toHaveURL(/#\/settings\/company$/);
});

test('cached reads are kept apart per company by the service worker', async ({ page }) => {
  await openAs(page, '/?company=HALDEN01#/overview', 'many');
  // The service worker takes control on the next load; read again so its cache fills.
  await page.evaluate(() => navigator.serviceWorker.ready);
  await page.reload();
  await expect(page.locator('.kpis')).toBeVisible();
  const apiCaches = () => page.evaluate(async () => (await caches.keys()).filter((name) => name.startsWith('ledgers-api-')));
  await expect.poll(async () => (await apiCaches()).length).toBeGreaterThan(0);
  const first = await apiCaches();

  await page.goto('/?company=FZDEMO02#/overview');
  await expect(page.locator('.kpis')).toBeVisible();
  await page.reload();
  await expect(page.locator('.kpis')).toBeVisible();
  await expect.poll(async () => (await apiCaches()).length).toBeGreaterThan(first.length);
});
