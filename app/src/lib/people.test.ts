import { describe, expect, it } from 'vitest';
import {
  acceptErrorKey,
  acceptPath,
  acceptTokenFrom,
  assignableRoles,
  capabilityColumns,
  capabilityLabelKey,
  inviteErrorKey,
  isAcceptPath,
  isPlausibleEmail,
  leaveAllowed,
  matrixRows,
  ownerCount,
  roleHelpKey,
  roleLabelKey,
  rowRights,
  transferSteps,
  type MemberLike,
} from './people';
import { en } from '../i18n/en';
import { de } from '../i18n/de';
import { id } from '../i18n/id';
import { ar } from '../i18n/ar';

const members: (MemberLike & { email: string })[] = [
  { personId: 'p_owner', email: 'owner@halden.test', role: 'owner' },
  { personId: 'p_admin', email: 'admin@halden.test', role: 'admin' },
  { personId: 'p_view', email: 'viewer@halden.test', role: 'viewer' },
];

describe('who may give which role', () => {
  it('owners may give every role, owner included, when inviting', () => {
    expect(assignableRoles('owner', true)).toEqual(['owner', 'admin', 'accountant', 'approver', 'viewer', 'auditor']);
  });

  it('owners never get owner from a role change; ownership moves by transfer', () => {
    expect(assignableRoles('owner', false)).toEqual(['admin', 'accountant', 'approver', 'viewer', 'auditor']);
  });

  it('admins never give owner, even when inviting', () => {
    expect(assignableRoles('admin', true)).not.toContain('owner');
    expect(assignableRoles('admin', false)).not.toContain('owner');
  });

  it('other roles give nothing', () => {
    for (const role of ['accountant', 'approver', 'viewer', 'auditor']) {
      expect(assignableRoles(role, true)).toEqual([]);
    }
    expect(assignableRoles(null, true)).toEqual([]);
  });
});

describe('what a row offers the viewer', () => {
  it('an owner can change and remove everyone else, owners included', () => {
    expect(rowRights('owner', 'p_owner', members[1])).toEqual({ changeRole: true, remove: true });
    expect(rowRights('owner', 'p_owner', { personId: 'p_x', role: 'owner' })).toEqual({ changeRole: true, remove: true });
  });

  it('an admin reaches everyone except owners', () => {
    expect(rowRights('admin', 'p_admin', members[2])).toEqual({ changeRole: true, remove: true });
    expect(rowRights('admin', 'p_admin', members[0])).toEqual({ changeRole: false, remove: false });
  });

  it('a viewer reaches nobody', () => {
    expect(rowRights('viewer', 'p_view', members[1])).toEqual({ changeRole: false, remove: false });
  });

  it('a row for yourself never offers a role change or removal', () => {
    expect(rowRights('owner', 'p_owner', members[0])).toEqual({ changeRole: false, remove: false });
    expect(rowRights('admin', 'p_admin', members[1])).toEqual({ changeRole: false, remove: false });
  });
});

describe('leaving', () => {
  it('is allowed unless you are the only owner', () => {
    expect(leaveAllowed(members, 'p_view')).toBe(true);
    expect(leaveAllowed(members, 'p_owner')).toBe(false);
    const twoOwners = [...members, { personId: 'p_second', email: 'second@halden.test', role: 'owner' }];
    expect(leaveAllowed(twoOwners, 'p_owner')).toBe(true);
    expect(ownerCount(twoOwners)).toBe(2);
  });

  it('is refused for someone not in the roster', () => {
    expect(leaveAllowed(members, 'p_nobody')).toBe(false);
  });
});

describe('ownership transfer order', () => {
  it('promotes the target to owner first, then steps the viewer down to admin', () => {
    expect(transferSteps('p_owner', 'p_view')).toEqual([
      { personId: 'p_view', role: 'owner' },
      { personId: 'p_owner', role: 'admin' },
    ]);
  });

  it('never demotes the viewer first', () => {
    const [first, second] = transferSteps('p_owner', 'p_admin');
    expect(first.role).toBe('owner');
    expect(second.personId).toBe('p_owner');
  });
});

describe('roles matrix model', () => {
  const roles = [
    { role: 'viewer', classes: ['read'] },
    { role: 'owner', classes: ['read', 'settings', 'keys', 'audit', 'approvals-read', 'approvals-decide', 'post', 'money-out', 'close'] },
    { role: 'auditor', classes: ['read', 'audit'] },
    { role: 'mystery', classes: ['read', 'future-class'] },
  ];

  it('lists the known capability columns in canonical order, then the unknown ones sorted', () => {
    expect(capabilityColumns(roles)).toEqual([
      'read',
      'settings',
      'keys',
      'audit',
      'approvals-read',
      'approvals-decide',
      'post',
      'money-out',
      'close',
      'future-class',
    ]);
  });

  it('orders roles by ROLE_ORDER with unknown roles last', () => {
    expect(matrixRows(roles).map((row) => row.role)).toEqual(['owner', 'viewer', 'auditor', 'mystery']);
  });

  it('marks exactly the granted capabilities, one cell per column', () => {
    const rows = matrixRows(roles);
    const auditor = rows.find((row) => row.role === 'auditor')!;
    expect(auditor.cells).toHaveLength(10);
    expect(auditor.cells.filter((cell) => cell.granted).map((cell) => cell.capability)).toEqual(['read', 'audit']);
    const viewer = rows.find((row) => row.role === 'viewer')!;
    expect(viewer.cells.filter((cell) => cell.granted).map((cell) => cell.capability)).toEqual(['read']);
  });
});

describe('labels', () => {
  it('maps every role and capability the server names to a message', () => {
    for (const role of ['owner', 'admin', 'accountant', 'approver', 'viewer', 'auditor']) {
      expect(roleLabelKey(role)).not.toBeNull();
      expect(roleHelpKey(role)).not.toBeNull();
    }
    for (const cap of ['read', 'settings', 'keys', 'audit', 'approvals-read', 'approvals-decide', 'post', 'money-out', 'close']) {
      expect(capabilityLabelKey(cap)).not.toBeNull();
    }
    expect(roleLabelKey('mystery')).toBeNull();
    expect(capabilityLabelKey('future-class')).toBeNull();
  });

  it('has a one-line explanation for each role that matches the brief, in English', () => {
    expect(en['people.role_help.owner']).toBe('Everything, including transferring ownership.');
    expect(en['people.role_help.admin']).toBe('Everything except changing who is an owner.');
    expect(en['people.role_help.accountant']).toMatch(/posts/i);
    expect(en['people.role_help.approver']).toMatch(/approval/i);
    expect(en['people.role_help.viewer']).toMatch(/reads the books/i);
    expect(en['people.role_help.auditor']).toMatch(/audit log/i);
  });

  it('gives the German, Indonesian and Arabic explanations their own words', () => {
    for (const key of ['people.role_help.owner', 'people.role_help.auditor', 'people.roles_intro'] as const) {
      expect(de[key]).not.toBe(en[key]);
      expect(id[key]).not.toBe(en[key]);
      expect(ar[key]).not.toBe(en[key]);
    }
  });
});

describe('invitation and accept refusals', () => {
  it('reads a 503 or mail-not-configured as "not set up"', () => {
    expect(inviteErrorKey({ status: 503, slug: null })).toBe('people.err_mail_not_configured');
    expect(inviteErrorKey({ status: 400, slug: 'mail-not-configured' })).toBe('people.err_mail_not_configured');
  });

  it('reads an unknown role and an existing member', () => {
    expect(inviteErrorKey({ status: 422, slug: 'orgs-unknown-role' })).toBe('people.err_unknown_role');
    expect(inviteErrorKey({ status: 409, slug: null })).toBe('people.err_already_member');
    expect(inviteErrorKey({ status: 500, slug: null })).toBeNull();
  });

  it('reads an expired or used invitation as gone, and another address as the wrong one', () => {
    expect(acceptErrorKey({ kind: 'not_found', status: 404, slug: 'orgs-invitation-gone' })).toBe('accept.err_gone');
    expect(acceptErrorKey({ kind: 'not_found', status: 404, slug: null })).toBe('accept.err_gone');
    expect(acceptErrorKey({ kind: 'forbidden', status: 403, slug: null })).toBe('accept.err_wrong_address');
    expect(acceptErrorKey({ kind: 'unauthorized', status: 401, slug: 'session-required' })).toBe('accept.err_signed_out');
    expect(acceptErrorKey({ kind: 'server', status: 500, slug: null })).toBeNull();
  });
});

describe('accept links', () => {
  it('reads the token from the query and ignores an empty one', () => {
    expect(acceptTokenFrom('?token=tok_abc')).toBe('tok_abc');
    expect(acceptTokenFrom('?token=%20tok%20')).toBe('tok');
    expect(acceptTokenFrom('?token=')).toBeNull();
    expect(acceptTokenFrom('')).toBeNull();
  });

  it('builds the return path with the token encoded, and recognises the accept page', () => {
    expect(acceptPath('a b&c')).toBe('/accept?token=a%20b%26c');
    expect(isAcceptPath('/accept')).toBe(true);
    expect(isAcceptPath('/accept/')).toBe(true);
    expect(isAcceptPath('/')).toBe(false);
    expect(isAcceptPath('/acceptance')).toBe(false);
  });
});

describe('email shape', () => {
  it('accepts an address and refuses a typo', () => {
    expect(isPlausibleEmail('buchhaltung@halden.example')).toBe(true);
    expect(isPlausibleEmail(' a@b.co ')).toBe(true);
    expect(isPlausibleEmail('not-an-address')).toBe(false);
    expect(isPlausibleEmail('two@@at.example')).toBe(false);
    expect(isPlausibleEmail('with space@x.example')).toBe(false);
  });
});
