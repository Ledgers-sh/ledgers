/**
 * Every screen at phone, tablet and desktop widths, in light and dark, against
 * the fake API. Each route must fit the viewport without page-level horizontal
 * scroll, keep every visible control fully inside the viewport and inside any
 * overflow-hidden ancestor that would cut it, and give phone-width controls a
 * tap target of at least 40px.
 */
import { expect, test, type Page } from '@playwright/test';
import { measure } from './responsive-measure';
import { mockApi, signInPerson, stopMockApi } from './mockApi';
import { companyState } from './mockCompanies';
import { groupsMock } from './mockGroups';

test.afterAll(stopMockApi);

const WIDTHS = [360, 390, 768, 1024, 1440];
const PHONE_WIDTHS = [360, 390];
const THEMES = ['light', 'dark'] as const;
const MIN_TAP = 40;

/** Routes that every nav item and the detail screens reach. */
const ROUTES: { name: string; hash: string; company?: string }[] = [
  { name: 'overview', hash: '#/overview' },
  { name: 'journal', hash: '#/journal' },
  { name: 'accounts', hash: '#/accounts' },
  { name: 'account detail', hash: '#/accounts/1510' },
  { name: 'business units', hash: '#/accounts/units' },
  { name: 'reports', hash: '#/reports' },
  { name: 'bills', hash: '#/bills' },
  { name: 'bank', hash: '#/bank' },
  { name: 'parties', hash: '#/parties' },
  { name: 'party page', hash: '#/parties/party-halden' },
  { name: 'party edit', hash: '#/parties/party-halden/edit' },
  { name: 'party import', hash: '#/parties/import' },
  { name: 'group directory', hash: '#/group', company: 'HALDEN01' },
  { name: 'group', hash: '#/group', company: 'FZIP01' },
  { name: 'group view', hash: '#/group/view', company: 'FZIP01' },
  { name: 'settings', hash: '#/settings' },
  { name: 'approvals', hash: '#/approvals' },
  { name: 'new entry', hash: '#/new' },
];

async function setTheme(page: Page, theme: 'light' | 'dark') {
  await signInPerson(page, theme);
}

test.beforeEach(async () => {
  await mockApi({ seed: true, person: true, units: true, bank: true, groups: true });
  companyState.companies = [
    { id: 'HALDEN01', name: 'Halden Works GmbH', baseCurrency: 'EUR', role: 'owner' },
    { id: 'FZIP01', name: 'Factory Zero IP Pte. Ltd.', baseCurrency: 'EUR', role: 'owner', logo_document_id: 'DOCFZIP' },
    { id: 'PTOP01', name: 'PT Operasional Nusantara', baseCurrency: 'EUR', role: 'owner' },
  ] as typeof companyState.companies;
  groupsMock.unitsEnabled = true;
  groupsMock.groups = [
    {
      id: 'FZGROUP01',
      name: 'Factory Zero Group',
      logo_document_id: 'DOCFZ',
      logo_company_id: 'FZIP01',
      members: [
        { company_id: 'FZIP01', kind: 'ip_holding', ownership_bp: 10000 },
        { company_id: 'PTOP01', kind: 'operating', ownership_bp: 10000 },
      ],
    },
  ] as typeof groupsMock.groups;
});

for (const route of ROUTES) {
  test(`${route.name} fits every width in light and dark`, async ({ page }) => {
    const failures: string[] = [];
    for (const theme of THEMES) {
      await setTheme(page, theme);
      for (const width of WIDTHS) {
        await page.setViewportSize({ width, height: 800 });
        await page.goto(`/${route.company ? `?company=${route.company}` : ''}${route.hash}`);
        await expect(page.locator('main')).toBeVisible();
        await page.waitForLoadState('networkidle');
        const tapMin = PHONE_WIDTHS.includes(width) ? MIN_TAP : 0;
        for (const problem of await measure(page, tapMin)) {
          failures.push(`[${route.name} ${width}px ${theme}] ${problem}`);
        }
      }
    }
    expect(failures, failures.join('\n')).toEqual([]);
  });
}

/** Pickers, sheets and dialogs open at phone widths and stay inside the screen. */
test('pickers, sheets and the command palette fit a phone screen', async ({ page }) => {
  const failures: string[] = [];
  const openers: { name: string; hash: string; open: (p: Page) => Promise<void>; popup: string }[] = [
    { name: 'company switcher', hash: '#/overview', open: (p) => p.locator('#company-switch').click(), popup: '.sel-popup' },
    { name: 'unit switcher', hash: '#/overview', open: (p) => p.locator('#unit-switch').click(), popup: '.sel-popup' },
    { name: 'period picker', hash: '#/overview', open: (p) => p.locator('.pp-trigger').click(), popup: '.pp-pop' },
    { name: 'language picker', hash: '#/overview', open: (p) => p.locator('#language-select').click(), popup: '.sel-popup' },
    { name: 'command palette', hash: '#/overview', open: (p) => p.locator('.palette-button').click(), popup: 'dialog[open], .palette' },
    { name: 'more sections', hash: '#/overview', open: (p) => p.locator('.bottom-more-toggle').click(), popup: '.bottom-more' },
    { name: 'account picker on a new entry', hash: '#/new', open: (p) => p.getByRole('combobox', { name: 'Line 1 account' }).click(), popup: '.sel-popup' },
  ];
  for (const theme of THEMES) {
    await setTheme(page, theme);
    for (const width of PHONE_WIDTHS) {
      await page.setViewportSize({ width, height: 760 });
      for (const opener of openers) {
        await page.goto(`/${opener.hash}`);
        await expect(page.locator('main')).toBeVisible();
        await page.waitForLoadState('networkidle');
        await opener.open(page);
        const popup = page.locator(opener.popup).filter({ visible: true }).first();
        await expect(popup, `${opener.name} opens`).toBeVisible();
        const box = await popup.boundingBox();
        const viewport = page.viewportSize();
        if (!box || !viewport) throw new Error(`${opener.name} has no box`);
        const label = `[${opener.name} ${width}px ${theme}]`;
        if (box.x < -0.5 || box.x + box.width > viewport.width + 0.5) failures.push(`${label} popup ${Math.round(box.x)}..${Math.round(box.x + box.width)} is off the screen`);
        if (box.y < -0.5 || box.y + box.height > viewport.height + 0.5) failures.push(`${label} popup ${Math.round(box.y)}..${Math.round(box.y + box.height)} is off the screen vertically`);
        for (const problem of await measure(page, 0)) failures.push(`${label} ${problem}`);
        await page.keyboard.press('Escape');
      }
    }
  }
  expect(failures, failures.join('\n')).toEqual([]);
});

/** The selected tab of every scrolling tab row is fully inside the viewport, centred on load. */
test('the active tab of each tab row is fully inside the viewport', async ({ page }) => {
  const failures: string[] = [];
  const screens = [
    { name: 'approvals', hash: '#/approvals', company: undefined },
    { name: 'accounts', hash: '#/accounts', company: undefined },
    { name: 'bank', hash: '#/bank', company: undefined },
    { name: 'group view', hash: '#/group/view', company: 'FZIP01' },
    { name: 'group agreements', hash: '#/group/agreements', company: 'FZIP01' },
  ];
  for (const theme of THEMES) {
    await setTheme(page, theme);
    for (const width of PHONE_WIDTHS) {
      await page.setViewportSize({ width, height: 800 });
      for (const screen of screens) {
        await page.goto(`/${screen.company ? `?company=${screen.company}` : ''}${screen.hash}`);
        await expect(page.locator('main')).toBeVisible();
        await page.waitForLoadState('networkidle');
        const active = page.locator('.tabs .tab[aria-selected="true"], .tabs .tab[aria-current="page"]').first();
        await expect(active, `${screen.name} has an active tab`).toBeVisible();
        const box = await active.boundingBox();
        if (!box) throw new Error(`${screen.name} active tab has no box`);
        if (box.x < -0.5 || box.x + box.width > width + 0.5) {
          failures.push(`[${screen.name} ${width}px ${theme}] active tab ${Math.round(box.x)}..${Math.round(box.x + box.width)} is cut off`);
        }
      }
    }
  }
  expect(failures, failures.join('\n')).toEqual([]);
});

/** On a phone the group view's figures sit two to a row, member figures and unit figures alike. */
test('the group view figures are two per row at 390px', async ({ page }) => {
  await setTheme(page, 'dark');
  await page.setViewportSize({ width: 390, height: 800 });
  await page.goto('/?company=FZIP01#/group/view');
  await expect(page.locator('main')).toBeVisible();
  await page.waitForLoadState('networkidle');
  const rows = await page.evaluate(() => {
    const tops = (els: Element[]) => els.map((el) => Math.round(el.getBoundingClientRect().top));
    const lefts = (els: Element[]) => els.map((el) => Math.round(el.getBoundingClientRect().left));
    const figures = Array.from(document.querySelectorAll('dl.figures')).map((dl) => ({
      count: dl.children.length,
      tops: tops(Array.from(dl.children)),
      lefts: lefts(Array.from(dl.children)),
    }));
    const units = Array.from(document.querySelectorAll('dl.unit-figure-values')).map((dl) => ({
      count: dl.children.length,
      tops: tops(Array.from(dl.children)),
      lefts: lefts(Array.from(dl.children)),
    }));
    return { figures, units };
  });
  expect(rows.figures.length).toBeGreaterThan(0);
  expect(rows.units.length).toBeGreaterThan(0);
  for (const group of [...rows.figures, ...rows.units]) {
    // Two per row: the first two share a top edge and sit side by side.
    expect(group.tops[0]).toBe(group.tops[1]);
    expect(group.lefts[1]).toBeGreaterThan(group.lefts[0]);
  }
  for (const figure of rows.figures) expect(figure.count).toBe(6);
});

/** Every card in a grid or row is the same size: cards that share a row have equal height and width (within 1px). */
const CARD_SELECTOR = [
  '.kpis > .kpi',
  '.tiles > .tile',
  '.unit-cards > li',
  '.bank-accounts > li',
  '.gallery > .gallery-card',
  '.view-columns > .member-column',
  '.grid-main > .panel',
  '.grid-three > .panel',
  '.figures > .figure',
].join(', ');
const CARD_SCREENS = [
  { name: 'overview', hash: '#/overview', company: undefined },
  { name: 'connected sources', hash: '#/settings/integrations', company: undefined },
  { name: 'group view', hash: '#/group/view', company: 'FZIP01' },
  { name: 'business units', hash: '#/accounts/units', company: undefined },
  { name: 'bank accounts', hash: '#/bank', company: undefined },
];

test('cards in the same row have equal height and width at 390, 768 and 1440', async ({ page }) => {
  const failures: string[] = [];
  await setTheme(page, 'light');
  for (const width of [390, 768, 1440]) {
    await page.setViewportSize({ width, height: 900 });
    for (const screen of CARD_SCREENS) {
      await page.goto(`/${screen.company ? `?company=${screen.company}` : ''}${screen.hash}`);
      await expect(page.locator('main')).toBeVisible();
      await page.waitForLoadState('networkidle');
      // The cards arrive with the fake API's data: wait for the first one before measuring.
      if (screen.name !== 'business units') await expect(page.locator(CARD_SELECTOR).first()).toBeVisible();
      const result = await page.evaluate((selector) => {
        const cards = Array.from(document.querySelectorAll<HTMLElement>(selector))
          .filter((el) => el.getClientRects().length > 0)
          .map((el) => ({ rect: el.getBoundingClientRect(), grid: el.parentElement?.className ?? '' }));
        // Rows are cards whose tops sit within 2px of each other inside the same parent grid.
        const rows = new Map<string, typeof cards>();
        for (const card of cards) {
          const key = `${card.grid}|${Math.round(card.rect.top / 2)}`;
          const list = rows.get(key) ?? [];
          list.push(card);
          rows.set(key, list);
        }
        const mismatches: string[] = [];
        for (const row of rows.values()) {
          if (row.length < 2) continue;
          const heights = row.map((c) => c.rect.height);
          const widths = row.map((c) => c.rect.width);
          const spread = (xs: number[]) => Math.max(...xs) - Math.min(...xs);
          if (spread(heights) > 1 || spread(widths) > 1) {
            mismatches.push(`row of ${row.length} in .${row[0].grid.split(' ')[0]}: heights ${heights.map(Math.round).join('/')}, widths ${widths.map(Math.round).join('/')}`);
          }
        }
        return { count: cards.length, mismatches };
      }, CARD_SELECTOR);
      if (screen.name !== 'business units' && result.count === 0) failures.push(`[${screen.name} ${width}px] no cards found`);
      for (const mismatch of result.mismatches) failures.push(`[${screen.name} ${width}px] ${mismatch}`);
    }
  }
  expect(failures, failures.join('\n')).toEqual([]);
});
