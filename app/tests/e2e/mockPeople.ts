/**
 * The People routes of the fake API, behaving like the server's people module
 * (GET /v1/auth/session, people, roles, invitations; the organization
 * module's invitation, role and membership commands). mockApi.ts calls
 * handlePeople before its own routes. The state is reset by mockApi() and is
 * exported so specs can set the signed-in role and read what was sent.
 */

export interface PeopleReply {
  status: number;
  /** Undefined for an empty response (204). */
  body?: unknown;
  problem?: boolean;
}

export interface PeopleMember {
  personId: string;
  email: string;
  role: string;
  invitedBy: string | null;
  joinedAt: string;
  lastActiveAt: string | null;
}

export interface PeopleInvitation {
  id: string;
  role: string;
  invitedBy: string | null;
  createdAt: string;
  expiresAt: string;
}

export interface PeopleState {
  /** The signed-in person's role in the company. */
  role: string;
  /** Signed in with no membership: /v1/auth/session answers 403 not-a-member. */
  notMember: boolean;
  /** Whether the server has mail configured for invitations. */
  mailConfigured: boolean;
  members: PeopleMember[];
  invitations: PeopleInvitation[];
  /** Every invitation mailed through the fake, newest last. */
  sentInvitations: { email: string; role: string }[];
  /** Every role change, in the order the server received it. */
  roleChanges: { personId: string; role: string }[];
  removed: string[];
  left: boolean;
  /** Invitation tokens accepted (the fake accepts any token except the two it refuses). */
  accepted: string[];
  /** Every request the People routes received, as "METHOD path". */
  requests: string[];
}

export const COMPANY_ID = 'HALDEN01';

/** The signed-in owner in the fixtures (see PERSON in mockApi.ts). */
export const SELF_ID = 'person_01';

const NOW = Date.UTC(2026, 9, 20, 9, 0, 0);

function seedMembers(): PeopleMember[] {
  return [
    { personId: SELF_ID, email: 'owner@example.test', role: 'owner', invitedBy: null, joinedAt: '2026-01-05T09:00:00Z', lastActiveAt: '2026-10-23T08:14:00Z' },
    { personId: 'person_02', email: 'buchhaltung@halden.example', role: 'admin', invitedBy: SELF_ID, joinedAt: '2026-02-11T10:20:00Z', lastActiveAt: '2026-10-22T16:02:00Z' },
    { personId: 'person_03', email: 'lea@halden.example', role: 'accountant', invitedBy: SELF_ID, joinedAt: '2026-03-02T08:45:00Z', lastActiveAt: null },
    { personId: 'person_04', email: 'revisor@halden.example', role: 'auditor', invitedBy: 'person_02', joinedAt: '2026-06-18T13:30:00Z', lastActiveAt: '2026-09-30T11:00:00Z' },
  ];
}

function seedInvitations(): PeopleInvitation[] {
  return [
    {
      id: 'inv_01HZKQ7M',
      role: 'approver',
      invitedBy: SELF_ID,
      createdAt: '2026-10-18T09:00:00Z',
      expiresAt: '2026-10-25T09:00:00Z',
    },
  ];
}

export const peopleState: PeopleState = freshState();

function freshState(): PeopleState {
  return {
    role: 'owner',
    notMember: false,
    mailConfigured: true,
    members: seedMembers(),
    invitations: seedInvitations(),
    sentInvitations: [],
    roleChanges: [],
    removed: [],
    left: false,
    accepted: [],
    requests: [],
  };
}

/** Puts the People fixtures back: called by mockApi() before each test. */
export function resetPeople(): void {
  Object.assign(peopleState, freshState());
}

/** The role matrix the server serves (GET /v1/auth/roles), in the server's role order. */
const ROLE_CLASSES: { role: string; classes: string[] }[] = [
  { role: 'owner', classes: ['read', 'settings', 'keys', 'audit', 'approvals-read', 'approvals-decide', 'post', 'money-out', 'close'] },
  { role: 'admin', classes: ['read', 'settings', 'keys', 'audit', 'approvals-read', 'approvals-decide', 'post', 'close'] },
  { role: 'accountant', classes: ['read', 'post', 'close'] },
  { role: 'approver', classes: ['read', 'approvals-read', 'approvals-decide'] },
  { role: 'viewer', classes: ['read'] },
  { role: 'auditor', classes: ['read', 'audit', 'approvals-read'] },
];

const ROLE_NAMES = ROLE_CLASSES.map((entry) => entry.role);

function problem(status: number, slug: string, title: string, detail: string): PeopleReply {
  return { status, problem: true, body: { type: `https://ledgers.sh/problems/${slug}`, title, status, detail } };
}

/** The people routes. Returns null for any path the people module does not serve. */
export function handlePeople(method: string, url: URL, bodyText: string, asPerson: boolean): PeopleReply | null {
  const path = url.pathname;
  if (!path.startsWith('/v1/auth/') && !path.startsWith('/v1/orgs/')) return null;
  peopleState.requests.push(`${method} ${path}`);
  const body = parse(bodyText);
  const managers = peopleState.role === 'owner' || peopleState.role === 'admin';

  if (method === 'GET' && path === '/v1/auth/session' && asPerson) {
    if (peopleState.notMember) {
      return problem(403, 'not-a-member', "You're not a member of this company yet", 'The signed-in account holds no membership in this company. Ask the owner for an invitation.');
    }
    return {
      status: 200,
      body: {
        personId: SELF_ID,
        email: 'owner@example.test',
        companyId: COMPANY_ID,
        role: peopleState.role,
        roles: [peopleState.role],
        csrfToken: 'csrf_test',
        expiresAt: null,
      },
    };
  }
  if (method === 'GET' && path === '/v1/auth/people') {
    return { status: 200, body: { members: peopleState.members, me: { role: peopleState.role }, canManage: managers } };
  }
  if (method === 'GET' && path === '/v1/auth/roles') {
    return { status: 200, body: { roles: ROLE_CLASSES } };
  }
  if (method === 'GET' && path === '/v1/auth/invitations') {
    if (!managers) return problem(403, 'people-manage-forbidden', 'Only an owner or admin can manage people', 'Inviting, changing roles and removing people is for owners and admins.');
    return { status: 200, body: { invitations: peopleState.invitations } };
  }
  const revoke = /^\/v1\/auth\/invitations\/([^/]+)$/.exec(path);
  if (method === 'DELETE' && revoke) {
    if (!managers) return problem(403, 'people-manage-forbidden', 'Only an owner or admin can manage people', 'Inviting, changing roles and removing people is for owners and admins.');
    const index = peopleState.invitations.findIndex((invitation) => invitation.id === revoke[1]);
    if (index === -1) return problem(404, 'invitation-not-found', 'No open invitation with that id', 'The invitation was already accepted, revoked or has lapsed.');
    peopleState.invitations.splice(index, 1);
    return { status: 204 };
  }
  if (method === 'POST' && path === `/v1/orgs/${COMPANY_ID}/invitations`) {
    const input = body as { email?: string; role?: string };
    if (!peopleState.mailConfigured) return problem(503, 'mail-not-configured', 'Mail is not configured', 'Invitations need the mail service.');
    if (!input.role || !ROLE_NAMES.includes(input.role)) return problem(422, 'orgs-unknown-role', 'Unknown role', `The role ${input.role ?? ''} is not one of the roles.`);
    if (input.role === 'owner' && peopleState.role !== 'owner') return problem(403, 'people-manage-forbidden', 'Only an owner can invite an owner', 'Admins cannot grant owner.');
    if (peopleState.members.some((member) => member.email === input.email)) return problem(409, 'orgs-already-member', 'Already a member', 'That address is already a member.');
    peopleState.sentInvitations.push({ email: input.email ?? '', role: input.role });
    peopleState.invitations.push({
      id: `inv_${peopleState.sentInvitations.length.toString().padStart(3, '0')}`,
      role: input.role,
      invitedBy: SELF_ID,
      createdAt: new Date(NOW).toISOString(),
      expiresAt: new Date(NOW + 7 * 86_400_000).toISOString(),
    });
    return { status: 201, body: { email: input.email, role: input.role } };
  }
  if (method === 'POST' && path === '/v1/orgs/invitations/accept') {
    const token = (body as { token?: string }).token ?? '';
    if (token === 'tok-gone') return problem(404, 'orgs-invitation-gone', 'Invitation gone', 'The invitation has expired or was already used.');
    if (token === 'tok-other') return problem(403, 'orgs-invitation-wrong-address', 'Wrong address', 'This invitation was sent to another address.');
    peopleState.accepted.push(token);
    peopleState.notMember = false;
    return { status: 200, body: { companyId: COMPANY_ID, role: 'viewer' } };
  }
  const member = /^\/v1\/orgs\/HALDEN01\/members\/([^/]+)$/.exec(path);
  if (member && method === 'PATCH') {
    if (!managers) return problem(403, 'forbidden', 'Forbidden', 'Only an owner or admin may change roles.');
    const target = peopleState.members.find((row) => row.personId === member[1]);
    if (!target) return problem(404, 'orgs-not-found', 'Not found', 'No such member.');
    const role = (body as { role?: string }).role ?? '';
    if (peopleState.role !== 'owner' && (target.role === 'owner' || role === 'owner')) {
      return problem(403, 'forbidden', 'Forbidden', 'Only an owner can grant or revoke owner.');
    }
    peopleState.roleChanges.push({ personId: target.personId, role });
    target.role = role;
    if (target.personId === SELF_ID) peopleState.role = role;
    return { status: 200, body: target };
  }
  if (member && method === 'DELETE') {
    if (!managers) return problem(403, 'forbidden', 'Forbidden', 'Only an owner or admin may remove people.');
    peopleState.removed.push(member[1]);
    peopleState.members = peopleState.members.filter((row) => row.personId !== member[1]);
    return { status: 204 };
  }
  if (method === 'POST' && path === `/v1/orgs/${COMPANY_ID}/leave`) {
    const self = peopleState.members.find((row) => row.personId === SELF_ID);
    const owners = peopleState.members.filter((row) => row.role === 'owner').length;
    if (self?.role === 'owner' && owners <= 1) return problem(409, 'orgs-last-owner', 'Last owner', 'The company must keep an owner.');
    peopleState.left = true;
    peopleState.members = peopleState.members.filter((row) => row.personId !== SELF_ID);
    return { status: 204 };
  }
  return null;
}

function parse(text: string): unknown {
  if (!text) return {};
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return {};
  }
}
