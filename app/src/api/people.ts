/**
 * The People calls: the roster and roles (any signed-in person), the invitations
 * (owners and admins), and the organization module's invitation, role and
 * membership commands. The app holds no token: every call sends the session cookie.
 */
import { request, type ClientConfig } from './client';

export interface MemberView {
  personId: string;
  email: string;
  role: string;
  /** Who sent the invitation this person joined by, as the server names them. */
  invitedBy: string | null;
  joinedAt: string;
  /** RFC 3339, or null when the person has never been active. */
  lastActiveAt: string | null;
}

export interface PeopleView {
  members: MemberView[];
  me: { role: string };
  canManage: boolean;
}

export interface RoleView {
  role: string;
  classes: string[];
}

export interface InvitationView {
  id: string;
  role: string;
  invitedBy: string | null;
  createdAt: string;
  expiresAt: string;
}

/** GET /v1/auth/people: the roster, the viewer's role and whether the viewer may manage people. */
export function peopleList(config: ClientConfig): Promise<PeopleView> {
  return request<PeopleView>(config, { method: 'GET', path: '/v1/auth/people' });
}

/** GET /v1/auth/roles: each role and the capabilities it carries. */
export function rolesList(config: ClientConfig): Promise<{ roles: RoleView[] }> {
  return request<{ roles: RoleView[] }>(config, { method: 'GET', path: '/v1/auth/roles' });
}

/** GET /v1/auth/invitations (owners and admins). The address is not returned; only a hash is kept. */
export function invitationsList(config: ClientConfig): Promise<{ invitations: InvitationView[] }> {
  return request<{ invitations: InvitationView[] }>(config, { method: 'GET', path: '/v1/auth/invitations' });
}

/** DELETE /v1/auth/invitations/{id} (owners and admins): revokes an open invitation. */
export function invitationRevoke(config: ClientConfig, id: string): Promise<void> {
  return request<void>(config, { method: 'DELETE', path: `/v1/auth/invitations/${encodeURIComponent(id)}` });
}

/** POST /v1/orgs/{companyId}/invitations: mails an invitation to `email` for `role`. */
export function invitationCreate(config: ClientConfig, companyId: string, input: { email: string; role: string }): Promise<unknown> {
  return request<unknown>(config, {
    method: 'POST',
    path: `/v1/orgs/${encodeURIComponent(companyId)}/invitations`,
    body: input,
  });
}

/** POST /v1/orgs/invitations/accept: joins the company the invitation names, for the signed-in person. */
export function invitationAccept(config: ClientConfig, token: string): Promise<unknown> {
  return request<unknown>(config, { method: 'POST', path: '/v1/orgs/invitations/accept', body: { token } });
}

/** PATCH /v1/orgs/{companyId}/members/{personId}: changes one member's role. */
export function memberSetRole(config: ClientConfig, companyId: string, personId: string, role: string): Promise<unknown> {
  return request<unknown>(config, {
    method: 'PATCH',
    path: `/v1/orgs/${encodeURIComponent(companyId)}/members/${encodeURIComponent(personId)}`,
    body: { role },
  });
}

/** DELETE /v1/orgs/{companyId}/members/{personId}: removes a member. */
export function memberRemove(config: ClientConfig, companyId: string, personId: string): Promise<void> {
  return request<void>(config, {
    method: 'DELETE',
    path: `/v1/orgs/${encodeURIComponent(companyId)}/members/${encodeURIComponent(personId)}`,
  });
}

/** POST /v1/orgs/{companyId}/leave: the signed-in person leaves the company. */
export function memberLeave(config: ClientConfig, companyId: string): Promise<unknown> {
  return request<unknown>(config, { method: 'POST', path: `/v1/orgs/${encodeURIComponent(companyId)}/leave` });
}
