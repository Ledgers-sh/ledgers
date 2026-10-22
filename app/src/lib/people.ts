/**
 * People helpers: the roles and what each may do, who may change whom, the
 * order an ownership transfer runs in, and how server refusals read. Pure
 * functions, so the screens and their tests share one definition. The server
 * still decides every rule; these only decide what the screen offers.
 */
import type { MessageKey } from '../i18n';

/** The roles, most powerful first: the order the People screen lists them in. */
export const ROLE_ORDER = ['owner', 'admin', 'accountant', 'approver', 'viewer', 'auditor'] as const;
export type RoleName = (typeof ROLE_ORDER)[number];

/** The capabilities the roles matrix shows, in the order the server names them. */
export const CAPABILITY_ORDER = [
  'read',
  'settings',
  'keys',
  'audit',
  'approvals-read',
  'approvals-decide',
  'post',
  'money-out',
  'close',
] as const;

const ROLE_LABEL: Record<RoleName, MessageKey> = {
  owner: 'people.role.owner',
  admin: 'people.role.admin',
  accountant: 'people.role.accountant',
  approver: 'people.role.approver',
  viewer: 'people.role.viewer',
  auditor: 'people.role.auditor',
};

const ROLE_HELP: Record<RoleName, MessageKey> = {
  owner: 'people.role_help.owner',
  admin: 'people.role_help.admin',
  accountant: 'people.role_help.accountant',
  approver: 'people.role_help.approver',
  viewer: 'people.role_help.viewer',
  auditor: 'people.role_help.auditor',
};

const CAPABILITY_LABEL: Record<(typeof CAPABILITY_ORDER)[number], MessageKey> = {
  read: 'people.cap.read',
  settings: 'people.cap.settings',
  keys: 'people.cap.keys',
  audit: 'people.cap.audit',
  'approvals-read': 'people.cap.approvals_read',
  'approvals-decide': 'people.cap.approvals_decide',
  post: 'people.cap.post',
  'money-out': 'people.cap.money_out',
  close: 'people.cap.close',
};

export function isRoleName(value: string): value is RoleName {
  return (ROLE_ORDER as readonly string[]).includes(value);
}

/** The label of a role, or null for a role the app does not know (the server's own word is shown then). */
export function roleLabelKey(role: string): MessageKey | null {
  return isRoleName(role) ? ROLE_LABEL[role] : null;
}

/** The one-line explanation of a role under the invite form. */
export function roleHelpKey(role: string): MessageKey | null {
  return isRoleName(role) ? ROLE_HELP[role] : null;
}

/** The label of a capability column, or null for one the app does not know yet. */
export function capabilityLabelKey(capability: string): MessageKey | null {
  return (CAPABILITY_ORDER as readonly string[]).includes(capability)
    ? CAPABILITY_LABEL[capability as (typeof CAPABILITY_ORDER)[number]]
    : null;
}

/** Owners and admins manage people; everyone else only reads the roster. */
export function canManagePeople(role: string | null | undefined): boolean {
  return role === 'owner' || role === 'admin';
}

/**
 * The roles a person with `viewerRole` may give. Admins never give owner.
 * Owner is offered only where `includeOwner` is set (inviting); changing a
 * role never grants ownership, which is what the transfer is for.
 */
export function assignableRoles(viewerRole: string | null | undefined, includeOwner: boolean): RoleName[] {
  if (!canManagePeople(viewerRole)) return [];
  return ROLE_ORDER.filter((role) => role !== 'owner' || (includeOwner && viewerRole === 'owner'));
}

export interface MemberLike {
  personId: string;
  role: string;
}

export function ownerCount(members: readonly MemberLike[]): number {
  return members.filter((member) => member.role === 'owner').length;
}

/** Leaving is offered unless it would leave the company with no owner. */
export function leaveAllowed(members: readonly MemberLike[], selfId: string): boolean {
  const self = members.find((member) => member.personId === selfId);
  if (!self) return false;
  return self.role !== 'owner' || ownerCount(members) > 1;
}

export interface RowRights {
  /** The row's role can be changed. */
  changeRole: boolean;
  /** The row's person can be removed. */
  remove: boolean;
}

/**
 * What the viewer may do to one row. Your own row offers Leave (see
 * leaveAllowed), never a role change or removal: those would be the way to
 * lock yourself out. Owners reach every other row; admins reach everyone but owners.
 */
export function rowRights(viewerRole: string | null | undefined, selfId: string, member: MemberLike): RowRights {
  if (member.personId === selfId) return { changeRole: false, remove: false };
  const reach = viewerRole === 'owner' || (viewerRole === 'admin' && member.role !== 'owner');
  return { changeRole: reach, remove: reach };
}

export interface RoleStep {
  personId: string;
  role: RoleName;
}

/**
 * An ownership transfer, in the order it must run: the target becomes owner
 * first, so the company always has an owner; then the viewer steps down to
 * admin. Demoting first could leave no owner at all.
 */
export function transferSteps(selfId: string, targetId: string): [RoleStep, RoleStep] {
  return [
    { personId: targetId, role: 'owner' },
    { personId: selfId, role: 'admin' },
  ];
}

export interface RoleClasses {
  role: string;
  classes: readonly string[];
}

/** The capability columns: the known ones in canonical order, then any the server adds, sorted. */
export function capabilityColumns(roles: readonly RoleClasses[]): string[] {
  const seen = new Set(roles.flatMap((role) => role.classes));
  const known = CAPABILITY_ORDER.filter((capability) => seen.has(capability));
  const extra = [...seen].filter((capability) => !(CAPABILITY_ORDER as readonly string[]).includes(capability)).sort();
  return [...known, ...extra];
}

export interface MatrixCell {
  capability: string;
  granted: boolean;
}

export interface MatrixRow {
  role: string;
  cells: MatrixCell[];
}

/** The roles matrix as rows (roles in ROLE_ORDER, unknown ones last) with one cell per capability column. */
export function matrixRows(roles: readonly RoleClasses[]): MatrixRow[] {
  const columns = capabilityColumns(roles);
  const rank = (role: string) => {
    const index = (ROLE_ORDER as readonly string[]).indexOf(role);
    return index === -1 ? ROLE_ORDER.length : index;
  };
  return [...roles]
    .sort((a, b) => rank(a.role) - rank(b.role))
    .map((role) => ({
      role: role.role,
      cells: columns.map((capability) => ({ capability, granted: role.classes.includes(capability) })),
    }));
}

/** A plain check of the address shape. The server validates for real; this only catches typos before a round trip. */
export function isPlausibleEmail(value: string): boolean {
  return /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(value.trim());
}

/** What a failed invitation means to the person sending it, or null for the generic wording. */
export function inviteErrorKey(error: { status: number | null; slug: string | null }): MessageKey | null {
  if (error.slug === 'mail-not-configured' || error.status === 503) return 'people.err_mail_not_configured';
  if (error.slug === 'orgs-unknown-role') return 'people.err_unknown_role';
  if (error.status === 409) return 'people.err_already_member';
  return null;
}

/** What a failed accept means to the person who followed the link, or null for the generic wording. */
export function acceptErrorKey(error: { kind: string; status: number | null; slug: string | null }): MessageKey | null {
  if (error.slug === 'orgs-invitation-gone' || error.status === 404 || error.status === 410) return 'accept.err_gone';
  if (error.status === 403) return 'accept.err_wrong_address';
  if (error.kind === 'unauthorized') return 'accept.err_signed_out';
  return null;
}

/** The invitation token in an accept link's query, or null when there is none. */
export function acceptTokenFrom(search: string): string | null {
  const token = new URLSearchParams(search).get('token')?.trim();
  return token ? token : null;
}

/** The app path of the accept page for a token: the return address after sign-in too. */
export function acceptPath(token: string): string {
  return `/accept?token=${encodeURIComponent(token)}`;
}

/** Whether a pathname is the accept page (with or without a trailing slash). */
export function isAcceptPath(pathname: string): boolean {
  return pathname.replace(/\/+$/, '') === '/accept';
}
