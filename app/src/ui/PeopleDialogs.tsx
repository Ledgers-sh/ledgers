import { useEffect, useState } from 'preact/hooks';
import { invitationCreate, invitationRevoke, memberLeave, memberRemove, memberSetRole, type InvitationView, type MemberView } from '../api/people';
import { ApiError } from '../api/client';
import { assignableRoles, inviteErrorKey, isPlausibleEmail, transferSteps } from '../lib/people';
import { t } from '../i18n';
import { Ltr } from '../i18n/Ltr';
import { useSession } from '../session';
import { ConfirmDialog, describeError } from './common';
import { Select } from './Select';
import { roleText } from './PeopleRoles';
import { FieldProblem } from './settings/common';

/** A failed people call as one sentence: the reason we know, else the server's own title. */
export function failureText(error: unknown): string {
  const key = error instanceof ApiError ? inviteErrorKey(error) : null;
  return key ? t(key) : describeError(error).title;
}

/** The result of an action: a message for the page to show once the dialog closes. */
interface DialogProps {
  open: boolean;
  onClose: () => void;
  companyId: string;
  /** Called with the words to show after success. */
  onDone: (text: string) => void;
}

/** Change one member's role. Owners may choose any role but owner; admins likewise. Ownership moves only by transfer. */
export function ChangeRoleDialog({
  open,
  member,
  viewerRole,
  companyId,
  onClose,
  onDone,
}: DialogProps & { member: MemberView | null; viewerRole: string }) {
  const { client } = useSession();
  const options = assignableRoles(viewerRole, false);
  const [role, setRole] = useState(member?.role ?? '');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);

  useEffect(() => {
    if (open && member) {
      setRole('');
      setError(null);
    }
  }, [open, member?.personId]);

  async function confirm() {
    if (!member || !role) return;
    setBusy(true);
    setError(null);
    try {
      await memberSetRole(client, companyId, member.personId, role);
      onDone(t('people.role_saved', { name: member.email, role: roleText(role) }));
      onClose();
    } catch (err) {
      setError(err);
    } finally {
      setBusy(false);
    }
  }

  return (
    <ConfirmDialog
      open={open}
      onClose={onClose}
      title={t('people.role_title', { name: member?.email ?? '' })}
      confirmLabel={t('people.role_confirm')}
      busy={busy}
      error={error}
      disabled={!role || role === member?.role}
      onConfirm={() => void confirm()}
    >
      <div class="field">
        <span id="role-change-label">{t('people.role_label')}</span>
        <Select
          id="role-change"
          labelledBy="role-change-label"
          value={role}
          placeholder={t('people.role_pick')}
          options={options.map((name) => ({ value: name, label: roleText(name) }))}
          onChange={setRole}
        />
      </div>
    </ConfirmDialog>
  );
}

/** Remove a member from the company. Their past entries stay in the books. */
export function RemoveDialog({ open, member, companyId, onClose, onDone }: DialogProps & { member: MemberView | null }) {
  const { client } = useSession();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);

  useEffect(() => {
    if (open) setError(null);
  }, [open]);

  async function confirm() {
    if (!member) return;
    setBusy(true);
    setError(null);
    try {
      await memberRemove(client, companyId, member.personId);
      onDone(t('people.removed', { name: member.email }));
      onClose();
    } catch (err) {
      setError(err);
    } finally {
      setBusy(false);
    }
  }

  return (
    <ConfirmDialog
      open={open}
      onClose={onClose}
      title={t('people.remove_title', { name: member?.email ?? '' })}
      confirmLabel={t('people.remove_confirm')}
      danger
      busy={busy}
      error={error}
      onConfirm={() => void confirm()}
    >
      <p>{t('people.remove_body')}</p>
    </ConfirmDialog>
  );
}

/** Leave the company. The page reloads afterwards, and the screen shows that the account is no longer a member. */
export function LeaveDialog({ open, companyId, onClose }: Omit<DialogProps, 'onDone'>) {
  const { client } = useSession();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);

  useEffect(() => {
    if (open) setError(null);
  }, [open]);

  async function confirm() {
    setBusy(true);
    setError(null);
    try {
      await memberLeave(client, companyId);
      window.location.reload();
    } catch (err) {
      setError(err);
      setBusy(false);
    }
  }

  return (
    <ConfirmDialog
      open={open}
      onClose={onClose}
      title={t('people.leave_title')}
      confirmLabel={t('people.leave_confirm')}
      danger
      busy={busy}
      error={error}
      onConfirm={() => void confirm()}
    >
      <p>{t('people.leave_body')}</p>
    </ConfirmDialog>
  );
}

/**
 * Send an invitation again. The server keeps only a hash of the address, so
 * the admin types it again. The old invitation is revoked first, then the new
 * one is sent to the typed address with the same role.
 */
export function ResendDialog({
  open,
  invitation,
  companyId,
  onClose,
  onDone,
  onHalf,
}: DialogProps & { invitation: InvitationView | null; onHalf: (text: string) => void }) {
  const { client } = useSession();
  const [email, setEmail] = useState('');
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);

  useEffect(() => {
    if (open) {
      setEmail('');
      setProblem(null);
      setError(null);
    }
  }, [open, invitation?.id]);

  async function confirm() {
    if (!invitation) return;
    if (!isPlausibleEmail(email)) {
      setProblem(t('settings.email_format'));
      document.getElementById('resend-email')?.focus();
      return;
    }
    const address = email.trim();
    setProblem(null);
    setBusy(true);
    setError(null);
    try {
      await invitationRevoke(client, invitation.id);
    } catch (err) {
      setError(err);
      setBusy(false);
      return;
    }
    try {
      await invitationCreate(client, companyId, { email: address, role: invitation.role });
      onDone(t('people.resent', { email: address }));
      onClose();
    } catch (err) {
      // The old invitation is already revoked: say so, so nobody assumes a mail went out.
      onHalf(`${t('people.resend_half')} ${failureText(err)}`);
      onClose();
    } finally {
      setBusy(false);
    }
  }

  return (
    <ConfirmDialog
      open={open}
      onClose={onClose}
      title={t('people.resend_title')}
      confirmLabel={t('people.resend_confirm')}
      busy={busy}
      error={error}
      onConfirm={() => void confirm()}
    >
      <p>{t('people.resend_body')}</p>
      <p class="muted">{t('people.resend_role', { role: roleText(invitation?.role ?? '') })}</p>
      <div class="field">
        <label class="label" for="resend-email">{t('people.resend_email')}</label>
        <input
          id="resend-email"
          class="input"
          type="email"
          dir="ltr"
          autoComplete="off"
          value={email}
          aria-invalid={problem ? 'true' : undefined}
          aria-describedby={problem ? 'resend-email-error' : undefined}
          onInput={(event) => setEmail((event.target as HTMLInputElement).value)}
        />
        <FieldProblem id="resend-email-error" message={problem} />
      </div>
    </ConfirmDialog>
  );
}

/**
 * Make another member the owner, then step down to admin. The order is the
 * point: the target is promoted first, so the company never has no owner.
 */
export function TransferDialog({
  open,
  target,
  selfId,
  companyId,
  onClose,
  onDone,
  onHalf,
}: DialogProps & { target: MemberView | null; selfId: string; onHalf: (text: string) => void }) {
  const { client } = useSession();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);

  useEffect(() => {
    if (open) setError(null);
  }, [open]);

  async function confirm() {
    if (!target) return;
    const [promote, stepDown] = transferSteps(selfId, target.personId);
    setBusy(true);
    setError(null);
    try {
      await memberSetRole(client, companyId, promote.personId, promote.role);
    } catch (err) {
      // Nothing changed yet: the company still has its owner.
      setError(err);
      setBusy(false);
      return;
    }
    try {
      await memberSetRole(client, companyId, stepDown.personId, stepDown.role);
      onDone(t('people.transferred', { name: target.email }));
      onClose();
    } catch (err) {
      onHalf(`${t('people.transfer_half')} ${failureText(err)}`);
      onClose();
    } finally {
      setBusy(false);
    }
  }

  return (
    <ConfirmDialog
      open={open}
      onClose={onClose}
      title={t('people.transfer_dialog_title', { name: target?.email ?? '' })}
      confirmLabel={t('people.transfer_confirm')}
      busy={busy}
      error={error}
      onConfirm={() => void confirm()}
    >
      <p>{t('people.transfer_body')}</p>
      {target && (
        <p class="muted">
          <Ltr>{target.email}</Ltr>
        </p>
      )}
    </ConfirmDialog>
  );
}
