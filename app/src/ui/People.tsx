import { Fragment } from 'preact';
import { useState } from 'preact/hooks';
import { invitationRevoke, invitationCreate, invitationsList, peopleList, rolesList, type InvitationView, type MemberView } from '../api/people';
import { ApiError } from '../api/client';
import { useAsync } from '../hooks';
import { intlLocale, t } from '../i18n';
import { Ltr } from '../i18n/Ltr';
import { useSession } from '../session';
import { assignableRoles, inviteErrorKey, isPlausibleEmail, leaveAllowed, roleHelpKey, rowRights } from '../lib/people';
import { Empty, ErrorBanner } from './common';
import { FormProblem, LoadingLine, SettingsPanel } from './settings/common';
import { Select } from './Select';
import { Icon } from './icons';
import { RoleBadge, RolesPanel, roleText } from './PeopleRoles';
import { ChangeRoleDialog, LeaveDialog, RemoveDialog, ResendDialog, TransferDialog, failureText } from './PeopleDialogs';

interface Notice {
  tone: 'ok' | 'error';
  text: string;
}

/** A timestamp in the reader's format; "Never" for none. */
function formatInstant(iso: string | null): string {
  if (!iso) return t('people.never');
  const when = new Date(iso);
  return Number.isNaN(when.getTime()) ? iso : when.toLocaleString(intlLocale(), { dateStyle: 'medium', timeStyle: 'short' });
}

/** A calendar date in the reader's format. */
function formatDay(iso: string): string {
  const when = new Date(iso);
  return Number.isNaN(when.getTime()) ? iso : when.toLocaleDateString(intlLocale(), { dateStyle: 'medium' });
}

/**
 * Settings → People. Everyone who can sign in reads the roster and the roles;
 * owners and admins invite, change roles and remove; an owner transfers
 * ownership. The server enforces every rule; these controls only offer what
 * the signed-in role may do.
 */
export function People() {
  const { client, person } = useSession();
  const roster = useAsync(() => peopleList(client), [client.baseUrl]);
  const roles = useAsync(() => rolesList(client), [client.baseUrl]);
  const view = roster.data;
  const manage = view?.canManage === true;
  const invitations = useAsync(() => (manage ? invitationsList(client) : Promise.resolve(null)), [client.baseUrl, manage]);

  const selfId = person?.personId ?? '';
  const companyId = person?.companyId ?? '';
  const myRole = view?.me.role ?? person?.role ?? '';
  const [notice, setNotice] = useState<Notice | null>(null);
  const [roleFor, setRoleFor] = useState<MemberView | null>(null);
  const [removeFor, setRemoveFor] = useState<MemberView | null>(null);
  const [resendFor, setResendFor] = useState<InvitationView | null>(null);
  const [transferFor, setTransferFor] = useState<MemberView | null>(null);
  const [leaving, setLeaving] = useState(false);

  /** Shows the outcome of an action and brings the lists up to date. */
  const done = (text: string) => {
    setNotice({ tone: 'ok', text });
    roster.reload();
    invitations.reload();
  };
  const halfDone = (text: string) => {
    setNotice({ tone: 'error', text });
    roster.reload();
    invitations.reload();
  };

  return (
    <>
      <div role="status" aria-live="polite">
        {notice && (
          <p class={`result ${notice.tone === 'ok' ? 'ok' : 'fail'}`}>
            <span>{notice.text}</span>
          </p>
        )}
      </div>

      <SettingsPanel id="people-members-title" title={t('people.members_title')}>
        <p class="muted">{t('people.members_intro')}</p>
        {roster.loading && !view && <LoadingLine />}
        {roster.error && !view && <ErrorBanner error={roster.error} onRetry={roster.reload} />}
        {view &&
          (view.members.length === 0 ? (
            <Empty title={t('people.empty_title')}>{t('people.empty_body')}</Empty>
          ) : (
            <div class="table-wrap">
              <table class="ledger cards people-roster">
                <caption class="sr-only">{t('people.members_title')}</caption>
                <thead>
                  <tr>
                    <th scope="col">{t('people.col_person')}</th>
                    <th scope="col">{t('people.col_role')}</th>
                    <th scope="col">{t('people.col_last_active')}</th>
                    <th scope="col">{t('people.col_joined')}</th>
                    <th scope="col">
                      <span class="sr-only">{t('people.col_actions')}</span>
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {view.members.map((member) => {
                    const isSelf = member.personId === selfId;
                    const rights = rowRights(myRole, selfId, member);
                    return (
                      <tr key={member.personId}>
                        <td class="main" data-label={t('people.col_person')}>
                          <span class="people-email">
                            <Ltr>{member.email}</Ltr>
                          </span>
                          {isSelf && <span class="people-you">{t('people.you')}</span>}
                        </td>
                        <td data-label={t('people.col_role')}>
                          <RoleBadge role={member.role} />
                        </td>
                        <td data-label={t('people.col_last_active')}>
                          {member.lastActiveAt ? (
                            formatInstant(member.lastActiveAt)
                          ) : (
                            <span class="muted">{t('people.never')}</span>
                          )}
                        </td>
                        <td data-label={t('people.col_joined')}>{formatDay(member.joinedAt)}</td>
                        <td class="actions people-actions">
                          {rights.changeRole && (
                            <button
                              type="button"
                              class="btn btn-sm"
                              aria-label={t('people.change_role_named', { name: member.email })}
                              onClick={() => setRoleFor(member)}
                            >
                              {t('people.change_role')}
                            </button>
                          )}
                          {rights.remove && (
                            <button
                              type="button"
                              class="btn btn-sm btn-danger"
                              aria-label={t('people.remove_named', { name: member.email })}
                              onClick={() => setRemoveFor(member)}
                            >
                              {t('people.remove')}
                            </button>
                          )}
                          {isSelf &&
                            (leaveAllowed(view.members, selfId) ? (
                              <button type="button" class="btn btn-sm btn-danger" onClick={() => setLeaving(true)}>
                                {t('people.leave')}
                              </button>
                            ) : (
                              <span class="muted people-small">{t('people.leave_blocked')}</span>
                            ))}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          ))}
      </SettingsPanel>

      {manage && (
        <SettingsPanel id="people-invite-title" title={t('people.invite_title')}>
          <p class="muted">{t('people.invite_intro')}</p>
          <InvitePanel viewerRole={myRole} companyId={companyId} onDone={done} />
        </SettingsPanel>
      )}

      {manage && (
        <SettingsPanel id="people-invitations-title" title={t('people.invitations_title')}>
          <p class="muted">{t('people.invitations_intro')}</p>
          {invitations.loading && !invitations.data && <LoadingLine />}
          {invitations.error && !invitations.data && <ErrorBanner error={invitations.error} onRetry={invitations.reload} />}
          {invitations.data &&
            (invitations.data.invitations.length === 0 ? (
              <Empty title={t('people.inv_empty_title')}>{t('people.inv_empty_body')}</Empty>
            ) : (
              <InvitationList
                invitations={invitations.data.invitations}
                onResend={setResendFor}
                onRevoked={() => {
                  setNotice({ tone: 'ok', text: t('people.revoked') });
                  invitations.reload();
                }}
                onError={(error) => setNotice({ tone: 'error', text: failureText(error) })}
              />
            ))}
        </SettingsPanel>
      )}

      {myRole === 'owner' && (
        <SettingsPanel id="people-transfer-title" title={t('people.transfer_title')}>
          <p class="muted">{t('people.transfer_intro')}</p>
          <TransferPanel members={view?.members ?? []} selfId={selfId} onPick={setTransferFor} />
        </SettingsPanel>
      )}

      {roles.data && <RolesPanel roles={roles.data.roles} />}
      {roles.error && !roles.data && (
        <SettingsPanel id="people-roles-title" title={t('people.roles_title')}>
          <ErrorBanner error={roles.error} onRetry={roles.reload} />
        </SettingsPanel>
      )}

      <ChangeRoleDialog
        open={roleFor !== null}
        member={roleFor}
        viewerRole={myRole}
        companyId={companyId}
        onClose={() => setRoleFor(null)}
        onDone={done}
      />
      <RemoveDialog
        open={removeFor !== null}
        member={removeFor}
        companyId={companyId}
        onClose={() => setRemoveFor(null)}
        onDone={done}
      />
      <LeaveDialog open={leaving} companyId={companyId} onClose={() => setLeaving(false)} />
      <ResendDialog
        open={resendFor !== null}
        invitation={resendFor}
        companyId={companyId}
        onClose={() => setResendFor(null)}
        onDone={done}
        onHalf={halfDone}
      />
      <TransferDialog
        open={transferFor !== null}
        target={transferFor}
        selfId={selfId}
        companyId={companyId}
        onClose={() => setTransferFor(null)}
        onDone={done}
        onHalf={halfDone}
      />
    </>
  );
}

/** Invite by email with a role. The explanation of each role is shown under the choice. */
function InvitePanel({ viewerRole, companyId, onDone }: { viewerRole: string; companyId: string; onDone: (text: string) => void }) {
  const { client } = useSession();
  const roleOptions = assignableRoles(viewerRole, viewerRole === 'owner');
  const [email, setEmail] = useState('');
  const [role, setRole] = useState<string>('viewer');
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const key = error instanceof ApiError ? inviteErrorKey(error) : null;

  async function submit(event: Event) {
    event.preventDefault();
    setError(null);
    if (!isPlausibleEmail(email)) {
      setProblem(t('settings.email_format'));
      document.getElementById('invite-email')?.focus();
      return;
    }
    setProblem(null);
    const address = email.trim();
    setBusy(true);
    try {
      await invitationCreate(client, companyId, { email: address, role });
      setEmail('');
      onDone(t('people.invited', { email: address }));
    } catch (err) {
      setError(err);
    } finally {
      setBusy(false);
    }
  }

  return (
    <form class="settings-form people-invite" onSubmit={(event) => void submit(event)} noValidate aria-label={t('people.invite_title')}>
      {error !== null &&
        (key ? (
          <div class="banner error" role="alert">
            <div class="grow">
              <strong>{t(key)}</strong>
            </div>
          </div>
        ) : (
          <FormProblem error={error} />
        ))}
      <div class="people-invite-fields">
        <label class="field">
          <span>{t('people.invite_email')}</span>
          <input
            id="invite-email"
            class="input"
            type="email"
            dir="ltr"
            autoComplete="off"
            value={email}
            aria-invalid={problem ? 'true' : undefined}
            aria-describedby={problem ? 'invite-email-error' : undefined}
            onInput={(event) => setEmail((event.target as HTMLInputElement).value)}
          />
          {problem && (
            <span id="invite-email-error" class="field-error">
              {problem}
            </span>
          )}
        </label>
        <div class="field">
          <span id="invite-role-label">{t('people.invite_role')}</span>
          <Select
            id="invite-role"
            labelledBy="invite-role-label"
            describedBy="invite-role-legend"
            value={role}
            onChange={setRole}
            options={roleOptions.map((name) => ({ value: name, label: roleText(name) }))}
          />
        </div>
      </div>
      <dl class="people-legend" id="invite-role-legend">
        {roleOptions.map((name) => {
          const help = roleHelpKey(name);
          return (
            <Fragment key={name}>
              <dt class={name === role ? 'current' : undefined}>{roleText(name)}</dt>
              <dd>{help ? t(help) : null}</dd>
            </Fragment>
          );
        })}
      </dl>
      <div class="form-actions">
        <button type="submit" class="btn btn-primary" disabled={busy} aria-busy={busy ? 'true' : undefined}>
          <Icon name="plus" />
          {busy ? t('common.working') : t('people.invite_submit')}
        </button>
      </div>
    </form>
  );
}

/** Pending invitations, with resend (type the address again) and revoke. */
function InvitationList({
  invitations,
  onResend,
  onRevoked,
  onError,
}: {
  invitations: InvitationView[];
  onResend: (invitation: InvitationView) => void;
  onRevoked: () => void;
  onError: (error: unknown) => void;
}) {
  const { client } = useSession();
  const [revoking, setRevoking] = useState<string | null>(null);

  async function revoke(invitation: InvitationView) {
    setRevoking(invitation.id);
    try {
      await invitationRevoke(client, invitation.id);
      onRevoked();
    } catch (err) {
      onError(err);
    } finally {
      setRevoking(null);
    }
  }

  return (
    <div class="table-wrap">
      <table class="ledger cards people-invitations">
        <caption class="sr-only">{t('people.invitations_title')}</caption>
        <thead>
          <tr>
            <th scope="col">{t('people.col_role')}</th>
            <th scope="col">{t('people.inv_col_invited_by')}</th>
            <th scope="col">{t('people.inv_col_expires')}</th>
            <th scope="col">
              <span class="sr-only">{t('people.col_actions')}</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {invitations.map((invitation) => {
            const sent = formatDay(invitation.createdAt);
            return (
              <tr key={invitation.id}>
                <td class="main" data-label={t('people.col_role')}>
                  <RoleBadge role={invitation.role} />
                </td>
                <td data-label={t('people.inv_col_invited_by')}>
                  {invitation.invitedBy ? <Ltr>{invitation.invitedBy}</Ltr> : <span class="muted">{t('common.empty_cell')}</span>}
                </td>
                <td data-label={t('people.inv_col_expires')}>{formatInstant(invitation.expiresAt)}</td>
                <td class="actions people-actions">
                  <button
                    type="button"
                    class="btn btn-sm"
                    aria-label={t('people.resend_named', { role: roleText(invitation.role), sent })}
                    disabled={revoking === invitation.id}
                    onClick={() => onResend(invitation)}
                  >
                    {t('people.resend')}
                  </button>
                  <button
                    type="button"
                    class="btn btn-sm btn-danger"
                    aria-label={t('people.revoke_named', { role: roleText(invitation.role), sent })}
                    disabled={revoking === invitation.id}
                    aria-busy={revoking === invitation.id ? 'true' : undefined}
                    onClick={() => void revoke(invitation)}
                  >
                    {t('people.revoke')}
                  </button>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

/** Owner only: choose the member who takes ownership, then confirm in the dialog. */
function TransferPanel({ members, selfId, onPick }: { members: MemberView[]; selfId: string; onPick: (member: MemberView) => void }) {
  const [target, setTarget] = useState('');
  const candidates = members.filter((member) => member.personId !== selfId);
  if (candidates.length === 0) return <p class="muted">{t('people.transfer_none')}</p>;
  const chosen = candidates.find((member) => member.personId === target);
  return (
    <div class="people-transfer">
      <div class="field">
        <span id="transfer-target-label">{t('people.transfer_select')}</span>
        <Select
          id="transfer-target"
          labelledBy="transfer-target-label"
          value={target}
          placeholder={t('people.transfer_pick')}
          onChange={setTarget}
          options={candidates.map((member) => ({ value: member.personId, label: member.email, hint: roleText(member.role) }))}
        />
      </div>
      <div class="form-actions">
        <button type="button" class="btn btn-primary" disabled={!chosen} onClick={() => chosen && onPick(chosen)}>
          {t('people.transfer_submit')}
        </button>
      </div>
    </div>
  );
}
