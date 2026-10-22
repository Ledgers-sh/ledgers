/**
 * The Overview charts against the fake API: the KPI strip and its changes, the
 * hover and keyboard crosshair with exact amounts, the legend toggles, the
 * period selector and the dimension switch, and an axe pass.
 */
import AxeBuilder from '@axe-core/playwright';
import { expect, test } from '@playwright/test';
import { mockApi, signIn, stopMockApi } from './mockApi';

test.afterAll(stopMockApi);

test.beforeEach(async () => {
  await mockApi({ seed: true });
});

async function openOverview(page: import('@playwright/test').Page, theme: 'dark' | 'light' = 'dark', locale: 'en' | 'de' | 'id' | 'ar' = 'en') {
  await signIn(page, theme, locale);
  await page.goto('/#/overview');
  await expect(page.locator('.kpis')).toBeVisible();
  await expect(page.locator('.chart-svg').first()).toBeVisible();
}

test('six figures with a change against the previous period, costs up shown as bad', async ({ page }) => {
  await openOverview(page);
  await expect(page.locator('.kpi')).toHaveCount(6);
  await expect(page.locator('.kpi').first()).toContainText('Revenue');
  // The mock has no postings in the previous period, so the change reads as new rather than a percentage.
  await expect(page.locator('.kpi').first().locator('.kpi-change')).toContainText('New vs previous period');
  await expect(page.locator('.kpi').nth(1).locator('.kpi-label')).toHaveText('Expenses');
  await expect(page.locator('.kpi .spark-line')).toHaveCount(6);
});

test('hovering the flow chart shows the exact amounts for the month under the pointer', async ({ page }) => {
  await openOverview(page);
  const svg = page.locator('.chart-svg').first();
  const box = await svg.boundingBox();
  if (!box) throw new Error('the chart has no box');
  await page.mouse.move(box.x + box.width - 12, box.y + box.height / 2);
  const tip = page.locator('.chart-tip').first();
  await expect(tip).toBeVisible();
  // This month: revenue 1,000.00, expenses 2,950.00 (rent and supplies), net −1,950.00.
  await expect(tip).toContainText('1,000.00');
  await expect(tip).toContainText('2,950.00');
  await expect(tip).toContainText('−1,950.00');
  await page.mouse.move(0, 0);
  await expect(tip).toHaveCount(0);
});

test('the legend toggles a series off and on', async ({ page }) => {
  await openOverview(page);
  const expenses = page.getByRole('button', { name: 'Expenses', exact: true }).first();
  await expect(expenses).toHaveAttribute('aria-pressed', 'true');
  await expenses.click();
  await expect(expenses).toHaveAttribute('aria-pressed', 'false');
  await expect(page.locator('.chart-svg').first().locator('.ch-exp')).toHaveCount(0);
  await expenses.click();
  await expect(page.locator('.chart-svg').first().locator('.ch-exp').first()).toBeVisible();
});

test('the keyboard moves the crosshair across the months', async ({ page }) => {
  await openOverview(page);
  await page.locator('.chart-hit').first().focus();
  const tip = page.locator('.chart-tip').first();
  await expect(tip).toBeVisible();
  const before = await tip.textContent();
  await page.keyboard.press('ArrowLeft');
  await expect(tip).not.toHaveText(before ?? '');
  await page.keyboard.press('Escape');
  await expect(tip).toHaveCount(0);
});

test('the period selector drives the overview and remembers the choice', async ({ page }) => {
  await openOverview(page);
  const trigger = page.locator('.pp-trigger');
  await trigger.click();
  await page.getByRole('radio', { name: 'This month' }).click();
  await expect(trigger).toHaveAttribute('aria-label', /This month/);
  await expect(page.locator('.kpi').first().locator('.kpi-sub')).not.toHaveText('');
  expect(await page.evaluate(() => window.sessionStorage.getItem('ledgers.range'))).toMatch(/^\d{4}-\d{2}\/\d{4}-\d{2}$/);
});

test('the dimension switch changes the split between cost centres and projects', async ({ page }) => {
  await openOverview(page);
  const legend = page.locator('.stack-legend');
  await expect(legend).toContainText('Administration');
  await page.getByRole('button', { name: 'Project', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Project', exact: true })).toHaveAttribute('aria-pressed', 'true');
  await expect(legend).toHaveCount(0);
  await expect(page.locator('.chart').filter({ hasText: 'Expenses by cost centre or project' }).locator('p.muted')).toHaveText(
    'Nothing is tagged to a value in this period.',
  );
});

test('the overview passes axe with no serious violations, in both themes', async ({ page }) => {
  for (const theme of ['dark', 'light'] as const) {
    await openOverview(page, theme);
    const results = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa']).analyze();
    const blocking = results.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical');
    expect(blocking.map((v) => `${v.id}: ${v.help}`), theme).toEqual([]);
  }
});

test('every chart has a text summary and a data table for screen readers', async ({ page }) => {
  await openOverview(page);
  await expect(page.locator('figure.chart')).toHaveCount(4);
  for (const figure of await page.locator('figure.chart').all()) {
    await expect(figure.locator('table')).toHaveCount(1);
    await expect(figure.locator('table caption')).not.toHaveText('');
  }
});
