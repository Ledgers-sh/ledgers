/**
 * People (Settings → People, the invitation link, the not-a-member screen)
 * against the fake API: the roster and roles read, what a viewer may not see,
 * inviting and its refusals, the ownership transfer order, and the accept
 * page. PEOPLE_SHOTS_DIR, when set, also writes the screenshots this file takes.
 */
import fs from 'node:fs';
import path from 'node:path';
import { expect, test, type Page } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';
import { mockApi, signInPerson, stopMockApi } from './mockApi';
import { peopleState, SELF_ID } from './mockPeople';

test.afterAll(stopMockApi);

const roster = (page: Page) => page.locator('table.people-roster');

/** Signs the fake owner in with another role: the session and the member's own row agree. */
function asRole(role: string) {
  peopleState.role = role;
  const self = peopleState.members.find((member) => member.personId === SELF_ID);
  if (self) self.role = role;
}

/** Opens the People section and waits for the roster to load. */
async function openPeople(page: Page) {
  await page.goto('/#/settings/people');
  // The first row is the same in every language, so this waits for the data, not a word.
  await expect(roster(page).locator('tbody tr').first()).toBeVisible();
}

test.describe('people', () => {
  test('the roster lists each member with role, last active and joined', async ({ page }) => {
    await mockApi({ person: true });
    await signInPerson(page);
    await openPeople(page);

    const accountant = roster(page).locator('tbody tr', { hasText: 'lea@halden.example' });
    await expect(accountant).toContainText('Accountant');
    await expect(accountant).toContainText('Never');
    const auditor = roster(page).locator('tbody tr', { hasText: 'revisor@halden.example' });
    await expect(auditor).toContainText('Auditor');
    await expect(auditor).toContainText('Sep');
    const me = roster(page).locator('tbody tr', { hasText: 'owner@example.test' });
    await expect(me).toContainText('Owner');
    await expect(me).toContainText('You');
  });

  test('the invite form explains every role under the role choice', async ({ page }) => {
    await mockApi({ person: true });
    await signInPerson(page);
    await openPeople(page);

    const legend = page.locator('#invite-role-legend');
    await expect(legend).toContainText('Everything, including transferring ownership.');
    await expect(legend).toContainText('Everything except changing who is an owner.');
    await expect(legend).toContainText('Posts and reverses entries');
    await expect(legend).toContainText('Decides approval requests.');
    await expect(legend).toContainText('Reads the books.');
    await expect(legend).toContainText('Reads the books, the audit log and exports.');

    // The role choice offers owner to an owner, and the explanation names the choice it is under.
    await page.getByRole('combobox', { name: /^Role\b/ }).click();
    await expect(page.getByRole('option', { name: 'Owner', exact: true })).toBeVisible();
    await page.keyboard.press('Escape');
  });

  test('a viewer sees the roster and roles but no invite, change or remove controls', async ({ page }) => {
    await mockApi({ person: true });
    asRole('viewer');
    await signInPerson(page);
    await openPeople(page);

    await expect(page.locator('#invite-email')).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Send invitation' })).toHaveCount(0);
    await expect(page.getByRole('heading', { name: 'Pending invitations' })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Change role' })).toHaveCount(0);
    await expect(page.getByRole('button', { name: /^Remove / })).toHaveCount(0);
    await expect(page.getByRole('heading', { name: 'Transfer ownership', exact: true })).toHaveCount(0);
    // A viewer may leave, since they are not the only owner.
    await expect(page.getByRole('button', { name: 'Leave company' })).toBeVisible();
    await expect(page.getByRole('table', { name: 'Roles and permissions' })).toBeVisible();
    expect(peopleState.requests).not.toContain('GET /v1/auth/invitations');
  });

  test('an account that is not a member is told to ask the owner for an invitation', async ({ page }) => {
    await mockApi({ person: true });
    peopleState.notMember = true;
    await signInPerson(page);
    await page.goto('/');

    await expect(page.getByRole('heading', { name: "You're not a member of this company yet" })).toBeVisible();
    await expect(page.getByText('Ask the owner for an invitation', { exact: false })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Sign out' })).toBeVisible();
  });

  test('accept: a signed-in invitee joins and is taken to the ledger', async ({ page }) => {
    await mockApi({ person: true });
    peopleState.notMember = true;
    await signInPerson(page);
    await page.goto('/accept?token=tok-good');

    await expect(page.getByRole('status').getByText('You have joined the company.')).toBeVisible();
    await expect.poll(() => peopleState.accepted).toEqual(['tok-good']);
    await expect(page).toHaveURL(/#\/overview/, { timeout: 8000 });
  });

  test('accept: signed out, the page sends the person to sign in and back to this invitation', async ({ page }) => {
    await mockApi();
    const signIns: URL[] = [];
    await page.route('**/v1/auth/login**', async (route) => {
      signIns.push(new URL(route.request().url()));
      await route.fulfill({ status: 200, contentType: 'text/html', body: '<p>Sign in</p>' });
    });
    await page.goto('/accept?token=tok-good');

    await expect.poll(() => signIns.length).toBe(1);
    expect(signIns[0].pathname).toBe('/v1/auth/login');
    expect(signIns[0].searchParams.get('return_to')).toBe('/accept?token=tok-good');
  });

  test('accept: an expired or used invitation says so', async ({ page }) => {
    await mockApi({ person: true });
    peopleState.notMember = true;
    await signInPerson(page);
    await page.goto('/accept?token=tok-gone');

    await expect(page.getByRole('alert').getByText('This invitation has expired or was already used.')).toBeVisible();
    expect(peopleState.accepted).toEqual([]);
  });

  test('accept: an invitation sent to another address offers sign-out', async ({ page }) => {
    await mockApi({ person: true });
    peopleState.notMember = true;
    await signInPerson(page);
    await page.goto('/accept?token=tok-other');

    await expect(page.getByRole('alert').getByText('This invitation was sent to a different email address.')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Sign out' })).toBeVisible();
  });

  test('inviting sends the invitation to the typed address with the chosen role', async ({ page }) => {
    await mockApi({ person: true });
    await signInPerson(page);
    await openPeople(page);

    await page.locator('#invite-email').fill('nina@halden.example');
    await page.getByRole('combobox', { name: /^Role\b/ }).click();
    await page.getByRole('option', { name: 'Approver', exact: true }).click();
    await page.getByRole('button', { name: 'Send invitation' }).click();

    await expect(page.getByRole('status').getByText('Invitation sent to nina@halden.example.')).toBeVisible();
    expect(peopleState.sentInvitations).toEqual([{ email: 'nina@halden.example', role: 'approver' }]);
    await expect(page.locator('#invite-email')).toHaveValue('');
  });

  test('a server without mail says invitations are not set up', async ({ page }) => {
    await mockApi({ person: true });
    peopleState.mailConfigured = false;
    await signInPerson(page);
    await openPeople(page);

    await page.locator('#invite-email').fill('nina@halden.example');
    await page.getByRole('button', { name: 'Send invitation' }).click();
    await expect(page.getByText('Invitations are not set up on this server.')).toBeVisible();
    expect(peopleState.sentInvitations).toEqual([]);
  });

  test('transfer: the target becomes owner first, then the owner steps down to admin', async ({ page }) => {
    await mockApi({ person: true });
    await signInPerson(page);
    await openPeople(page);

    await page.getByRole('combobox', { name: /^New owner\b/ }).click();
    await page.getByRole('option', { name: /buchhaltung@halden\.example/ }).click();
    await page.getByRole('button', { name: 'Transfer ownership…' }).click();
    const dialog = page.getByRole('dialog', { name: 'Make buchhaltung@halden.example the owner?' });
    await expect(dialog).toBeVisible();
    await dialog.getByRole('button', { name: 'Transfer ownership', exact: true }).click();

    await expect(page.getByRole('status').getByText('buchhaltung@halden.example is now an owner. You are now an admin.')).toBeVisible();
    expect(peopleState.roleChanges).toEqual([
      { personId: 'person_02', role: 'owner' },
      { personId: SELF_ID, role: 'admin' },
    ]);
  });

  test('removing a member asks first, then calls the company member route', async ({ page }) => {
    await mockApi({ person: true });
    await signInPerson(page);
    await openPeople(page);

    await page.getByRole('button', { name: 'Remove lea@halden.example' }).click();
    const dialog = page.getByRole('dialog', { name: 'Remove lea@halden.example from this company?' });
    await expect(dialog).toBeVisible();
    expect(peopleState.removed).toEqual([]);
    await dialog.getByRole('button', { name: 'Remove person', exact: true }).click();
    await expect(page.getByRole('status').getByText('lea@halden.example was removed.')).toBeVisible();
    expect(peopleState.removed).toEqual(['person_03']);
  });

  test('the People page has no serious accessibility violations', async ({ page }) => {
    await mockApi({ person: true });
    await signInPerson(page);
    await openPeople(page);
    const results = await new AxeBuilder({ page }).include('#main').analyze();
    const serious = results.violations.filter((violation) => violation.impact === 'serious' || violation.impact === 'critical');
    expect(serious.map((violation) => violation.id)).toEqual([]);
  });
});

const shotsDir = process.env.PEOPLE_SHOTS_DIR;

test.describe('people screenshots', () => {
  test.skip(!shotsDir, 'set PEOPLE_SHOTS_DIR to write the screenshots');
  test.beforeAll(() => {
    fs.mkdirSync(shotsDir!, { recursive: true });
  });

  const shots = [
    { name: 'desktop-1440', width: 1440, height: 900 },
    { name: 'phone-390', width: 390, height: 844 },
  ];

  for (const theme of ['dark', 'light'] as const) {
    for (const shot of shots) {
      test(`People ${theme} ${shot.name}`, async ({ page }) => {
        await mockApi({ person: true });
        await page.setViewportSize({ width: shot.width, height: shot.height });
        await signInPerson(page, theme);
        await openPeople(page);
        await page.screenshot({ path: path.join(shotsDir!, `people-${theme}-${shot.name}.png`), fullPage: true });
      });

      test(`Not a member ${theme} ${shot.name}`, async ({ page }) => {
        await mockApi({ person: true });
        peopleState.notMember = true;
        await page.setViewportSize({ width: shot.width, height: shot.height });
        await signInPerson(page, theme);
        await page.goto('/');
        await expect(page.getByRole('heading', { name: "You're not a member of this company yet" })).toBeVisible();
        await page.screenshot({ path: path.join(shotsDir!, `not-member-${theme}-${shot.name}.png`), fullPage: true });
      });
    }
  }

  test('People Arabic right-to-left phone 390', async ({ page }) => {
    await mockApi({ person: true });
    await page.setViewportSize({ width: 390, height: 844 });
    await signInPerson(page, 'dark', 'ar');
    await openPeople(page);
    expect(await page.evaluate(() => document.documentElement.dir)).toBe('rtl');
    await page.screenshot({ path: path.join(shotsDir!, 'people-ar-rtl-phone-390.png'), fullPage: true });
  });
});
