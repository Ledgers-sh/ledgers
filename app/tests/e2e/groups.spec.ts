/**
 * The group screens against the fake API (mockGroups.ts): the Group nav and
 * its members, creating an agreement, run, post (both or neither), a failed
 * post with the server's reason and a retry, settle, the reconciliation's open
 * month and differences, the group view's consolidation note, and the owner
 * rule. Also axe checks on the group pages.
 */
import AxeBuilder from '@axe-core/playwright';
import { expect, test, type Page } from '@playwright/test';
import { groupsMock } from './mockGroups';
import { mockApi, signIn, signInPerson, stopMockApi } from './mockApi';
import { companyState } from './mockCompanies';

test.afterAll(stopMockApi);

const CURRENT_MONTH = new Date().toISOString().slice(0, 7);

/** Opens an option of one Select by its trigger's id (the form's own ids, such as licensor-bank). */
async function pick(page: Page, triggerId: string, option: string | RegExp) {
  await page.locator(`#${triggerId}`).click();
  await page.getByRole('option', { name: option }).click();
}

/** Opens a group screen as the person, in the company the URL names (the default: the IP company). */
async function openGroup(page: Page, hash = '#/group', company = 'FZIP01') {
  const fragment = hash.startsWith('/#') ? hash.slice(1) : hash;
  await page.goto(`/?company=${company}${fragment}`);
}

test.describe('group: the Group screen', () => {
  test('shows the Group nav to a person and lists the members with their type and ownership', async ({ page }) => {
    await mockApi({ seed: true, person: true, groups: true });
    await signInPerson(page);
    await openGroup(page);
    await expect(page.getByRole('link', { name: 'Group', exact: true })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'Factory Zero Group', level: 2 })).toBeVisible();
    const table = page.getByRole('table', { name: 'Member companies' });
    await expect(table).toContainText('Factory Zero IP Pte. Ltd.');
    await expect(table).toContainText('IP holding');
    await expect(table).toContainText('PT Operasional Nusantara');
    await expect(table).toContainText('Operating');
    await expect(table).toContainText('60');
  });

  test('says why the group cannot be changed when the person is not an owner of a member', async ({ page }) => {
    await mockApi({ seed: true, person: true, groups: true });
    await signInPerson(page);
    companyState.companies = companyState.companies.map((company) =>
      company.id === 'PTOP01' ? { ...company, role: 'viewer' } : company,
    );
    await openGroup(page);
    const edit = page.getByRole('button', { name: 'Change group' });
    await expect(edit).toBeDisabled();
    await expect(page.getByText('You are not an owner of PT Operasional Nusantara.')).toBeVisible();
  });

  test('marks a member the person cannot see as hidden, never by name', async ({ page }) => {
    await mockApi({ seed: true, person: true, groups: true });
    groupsMock.groups[0]?.members.push({ company_id: 'KESTREL9', kind: 'holding', ownership_bp: 1000 });
    await signInPerson(page);
    await openGroup(page);
    await expect(page.getByText('Some member companies are not shown')).toBeVisible();
    await expect(page.getByText('Kestrel Holdings Ltd')).toHaveCount(0);
  });

  test('an agent key gets no Group nav item', async ({ page }) => {
    await mockApi({ seed: true, groups: true });
    await signIn(page);
    await page.goto('/#/overview');
    await expect(page.getByRole('link', { name: 'Group', exact: true })).toHaveCount(0);
  });
});

test.describe('group: agreements', () => {
  test('creates a royalty agreement with the withholding from the payer pack and the country notes', async ({ page }) => {
    await mockApi({ seed: true, person: true, groups: true });
    await signInPerson(page);
    await openGroup(page, '#/group/agreements/new');
    await expect(page.getByRole('heading', { name: 'New agreement', level: 1 })).toBeVisible();
    await pick(page, 'agreement-licensee', 'PT Operasional Nusantara');
    await pick(page, 'licensor-royalty', /4060 Royalty income/);
    await pick(page, 'licensor-ic', /1200 Intercompany receivable/);
    await pick(page, 'licensor-bank', /1000 Bank/);
    await pick(page, 'licensee-royalty', /6100 Royalty expense/);
    await pick(page, 'licensee-ic', /2070 Intercompany payable/);
    await pick(page, 'licensee-wht', /2060 Withholding payable/);
    await pick(page, 'licensee-bank', /1000 Bank/);
    const wht = page.locator('.wht-panel');
    await expect(wht).toContainText('20%');
    await expect(wht).toContainText("From the payer's country pack");
    await expect(page.getByRole('link', { name: 'DJP, PPh Pasal 26 (Kredit Pajak)' }).first()).toHaveAttribute('href', 'https://pajak.go.id/en/node/34286');
    await expect(page.locator('.country-notes')).toContainText('Advisory only, not tax advice');
    await page.getByLabel('Treaty rate (%), if one applies').fill('10');
    await expect(wht).toContainText('10%');
    await expect(wht).toContainText('Treaty rate you entered');
    await page.getByRole('button', { name: 'Create agreement' }).click();
    await expect(page.getByRole('heading', { name: 'Royalty', level: 1 })).toBeVisible();
    expect(groupsMock.writes.some((entry) => /^agreement\.create AG-NEW-/.test(entry))).toBe(true);
  });

  test('the list shows parties, basis, rate and withholding for the company', async ({ page }) => {
    await mockApi({ seed: true, person: true, groups: true });
    await signInPerson(page);
    await openGroup(page, '#/group/agreements');
    const row = page.getByRole('row', { name: /Royalty/ });
    await expect(row).toContainText('Factory Zero IP Pte. Ltd.');
    await expect(row).toContainText('PT Operasional Nusantara');
    await expect(row).toContainText('% of licensee revenue');
    await expect(row).toContainText('5%');
    await expect(row).toContainText('20%');
    await expect(row).toContainText('Active');
  });

  test('runs a month, reviews both sides side by side, and posts both or neither', async ({ page }) => {
    await mockApi({ seed: true, person: true, groups: true });
    await signInPerson(page);
    await openGroup(page, '#/group/agreements/AG-ID-1');
    await page.getByRole('button', { name: 'Run period' }).click();
    await expect(page.locator('.timeline-row.state-draft')).toHaveCount(1);
    await page.getByRole('button', { name: 'Review and post' }).first().click();
    const dialog = page.getByRole('dialog');
    await expect(dialog.getByRole('region', { name: 'Licensor invoice' })).toContainText('Intercompany receivable');
    await expect(dialog.getByRole('region', { name: 'Licensee bill' })).toContainText('Withholding payable');
    await expect(dialog.getByRole('region', { name: 'Licensee bill' })).toContainText('Balanced');
    await dialog.getByRole('button', { name: 'Post both sides' }).click();
    await expect(dialog).toBeHidden();
    expect(groupsMock.writes).toContain(`agreement.post AG-ID-1 ${CURRENT_MONTH}`);
    await expect(page.getByText('Posted').first()).toBeVisible();
  });

  test('a failed post shows the server reason, reverses both sides, and a retry posts them', async ({ page }) => {
    await mockApi({ seed: true, person: true, groups: true });
    groupsMock.failNextPost = CURRENT_MONTH;
    await signInPerson(page);
    await openGroup(page, '#/group/agreements/AG-ID-1');
    await page.getByRole('button', { name: 'Run period' }).click();
    await page.getByRole('button', { name: 'Review and post' }).first().click();
    const dialog = page.getByRole('dialog');
    await dialog.getByRole('button', { name: 'Post both sides' }).click();
    await expect(dialog.getByRole('alert')).toContainText("account 2070 is locked");
    await dialog.getByRole('button', { name: 'Cancel' }).click();
    const failure = page.getByRole('alert').filter({ hasText: 'Licensee failed:' });
    await expect(failure).toContainText('Licensee failed:');
    await expect(failure).toContainText('account 2070 is locked');
    await page.getByRole('button', { name: 'Retry posting' }).first().click();
    await page.getByRole('dialog').getByRole('button', { name: 'Retry posting' }).click();
    await expect(page.getByRole('dialog')).toBeHidden();
    expect(groupsMock.writes).toContain(`agreement.post AG-ID-1 ${CURRENT_MONTH}`);
    await expect(page.getByRole('alert').filter({ hasText: 'locked' })).toHaveCount(0);
  });

  test('settles a posted month with each side bank account in the dialog', async ({ page }) => {
    await mockApi({ seed: true, person: true, groups: true });
    await signInPerson(page);
    await openGroup(page, '#/group/agreements/AG-ID-1');
    await page.getByRole('button', { name: /^Settle / }).first().click();
    const dialog = page.getByRole('dialog');
    await expect(dialog).toContainText('Bank account in this company: 1000');
    await expect(dialog.getByRole('region', { name: 'Licensor receipt' })).toContainText('Withholding receivable');
    await expect(dialog.getByRole('region', { name: 'Licensee payment' })).toContainText('Bank');
    await dialog.getByRole('button', { name: 'Settle' }).click();
    await expect(dialog).toBeHidden();
    expect(groupsMock.writes.some((entry) => entry.startsWith('agreement.settle AG-ID-1'))).toBe(true);
  });

  test('an owner-less viewer sees the agreement without the run, post or settle actions', async ({ page }) => {
    await mockApi({ seed: true, person: true, groups: true });
    companyState.companies = companyState.companies.map((company) =>
      company.id === 'FZIP01' ? { ...company, role: 'viewer' } : company,
    );
    await signInPerson(page);
    await openGroup(page, '#/group/agreements/AG-ID-1');
    await expect(page.getByRole('button', { name: 'Run period' })).toBeDisabled();
    await expect(page.getByRole('button', { name: /^Settle / }).first()).toBeDisabled();
    await expect(page.getByText(/Posting needs an owner, an admin or an accountant/).first()).toBeVisible();
  });
});

test.describe('group: reconciliation and the group view', () => {
  test('marks the open month and any difference between the pair', async ({ page }) => {
    await mockApi({ seed: true, person: true, groups: true });
    await signInPerson(page);
    await openGroup(page, '#/group/reconciliation');
    const pair = page.getByRole('region', { name: /Factory Zero IP Pte\. Ltd\./ });
    await expect(pair.getByText('Open month: Feb 2026')).toBeVisible();
    await expect(pair.locator('tr.open-month')).toContainText('Feb 2026');
    await expect(pair.getByText('Balanced', { exact: true })).toBeVisible();
    groupsMock.reconDifference = 1250;
    await page.reload();
    await expect(pair.getByText('Difference', { exact: true })).toBeVisible();
    await expect(pair.locator('.recon-figure.flagged').first()).toContainText('Differs');
  });

  test('the group view puts companies side by side and says consolidation is a Ledgers Cloud feature', async ({ page }) => {
    await mockApi({ seed: true, person: true, groups: true });
    await signInPerson(page);
    await openGroup(page, '#/group/view');
    await expect(page.getByText('Combined consolidation is a Ledgers Cloud feature')).toBeVisible();
    await expect(page.getByRole('heading', { name: 'Factory Zero IP Pte. Ltd.' })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'PT Operasional Nusantara' })).toBeVisible();
    await expect(page.getByRole('button', { name: /consolidat/i })).toHaveCount(0);
  });
});

test.describe('group: accessibility', () => {
  const screens = [
    { name: 'group', hash: '#/group' },
    { name: 'agreements', hash: '#/group/agreements' },
    { name: 'agreement', hash: '#/group/agreements/AG-ID-1' },
    { name: 'agreement form', hash: '#/group/agreements/new' },
    { name: 'reconciliation', hash: '#/group/reconciliation' },
    { name: 'group view', hash: '#/group/view' },
  ];
  for (const screen of screens) {
    test(`${screen.name} has no serious or critical axe violations (en and ar)`, async ({ page }) => {
      for (const locale of ['en', 'ar'] as const) {
        await mockApi({ seed: true, person: true, groups: true });
        await signInPerson(page, 'light', locale);
        await openGroup(page, screen.hash);
        await expect(page.getByRole('heading', { level: 1 }).first()).toBeVisible();
        const results = await new AxeBuilder({ page })
          .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa'])
          .analyze();
        const blocking = results.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical');
        expect(blocking.map((v) => `${v.impact} ${v.id}: ${v.help}`), `${screen.name} ${locale}`).toEqual([]);
      }
    });
  }
});

