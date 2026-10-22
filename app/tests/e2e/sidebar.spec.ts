/**
 * The left sidebar: its background is as tall as the page, so it never ends
 * partway down a long page, and its content stays inside the window while the
 * page scrolls (top and end).
 */
import { expect, test } from '@playwright/test';
import { mockApi, signIn, stopMockApi } from './mockApi';

test.afterAll(stopMockApi);

test('the sidebar fills the page height and its content sticks to the window', async ({ page }) => {
  await mockApi({ seed: true, extraEntries: 120 });
  await signIn(page, 'dark');
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto('/#/journal');
  await expect(page.locator('.sidebar')).toBeVisible();
  const measure = () =>
    page.evaluate(() => {
      const column = (document.querySelector('.sidebar') as HTMLElement).getBoundingClientRect();
      const inner = (document.querySelector('.sidebar-inner') as HTMLElement).getBoundingClientRect();
      return {
        columnHeight: column.height,
        columnTop: column.top + window.scrollY,
        innerTop: inner.top,
        innerBottom: inner.bottom,
        viewport: window.innerHeight,
        pageHeight: document.documentElement.scrollHeight,
      };
    });
  const top = await measure();
  expect(top.pageHeight).toBeGreaterThan(top.viewport * 2);
  // The column covers the whole page, not just the first window.
  expect(top.columnHeight).toBeGreaterThanOrEqual(top.pageHeight - 1);
  expect(top.innerTop).toBeGreaterThanOrEqual(0);
  expect(top.innerBottom).toBeLessThanOrEqual(top.viewport + 1);

  await page.evaluate(() => window.scrollTo(0, document.documentElement.scrollHeight));
  await page.waitForTimeout(100);
  const end = await measure();
  expect(end.innerTop).toBeGreaterThanOrEqual(0);
  expect(end.innerBottom).toBeLessThanOrEqual(end.viewport + 1);
});
