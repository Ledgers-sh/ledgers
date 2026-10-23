/**
 * The two dialogs on a party's page that a person (owner or admin) confirms:
 * a bank-details change, which the server records with its reason, and a merge,
 * which shows what moves before anything is written.
 */
import { useState } from 'preact/hooks';
import { partyMerge, partySetBank, partyValidate, type Party, type PartyDetail } from '../api/parties';
import { t } from '../i18n';
import { Ltr } from '../i18n/Ltr';
import { useSession } from '../session';
import { ConfirmDialog } from './common';
import { FormProblem } from './settings/common';
import { Field } from './PartyFields';
import { PartyPicker } from './PartyPicker';
import { compactIban, fieldErrors, mergePreview } from '../lib/parties';

export function BankChangeDialog({
  party,
  open,
  onClose,
  onSaved,
}: {
  party: Party;
  open: boolean;
  onClose: () => void;
  onSaved: (party: Party) => void;
}) {
  const { client } = useSession();
  const [iban, setIban] = useState(party.iban ?? '');
  const [bic, setBic] = useState(party.bic ?? '');
  const [reason, setReason] = useState('');
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<unknown>(null);

  const checkBank = async () => {
    try {
      const result = await partyValidate(client, { id: party.id, iban: compactIban(iban), bic: bic.trim() });
      setErrors(fieldErrors(result.errors));
      return result.valid;
    } catch (error) {
      setProblem(error);
      return false;
    }
  };

  const save = async () => {
    setProblem(null);
    if (reason.trim() === '') {
      setErrors({ reason: t('parties.bank_reason_required') });
      return;
    }
    if (!(await checkBank())) return;
    setBusy(true);
    try {
      const result = await partySetBank(client, party.id, { iban: compactIban(iban), bic: bic.trim() || null, reason: reason.trim() });
      onSaved(result.party);
      onClose();
    } catch (error) {
      setProblem(error);
    } finally {
      setBusy(false);
    }
  };

  return (
    <ConfirmDialog
      open={open}
      onClose={onClose}
      title={t('parties.bank_title')}
      confirmLabel={t('parties.bank_confirm')}
      onConfirm={save}
      busy={busy}
      danger
      error={problem}
    >
      <div class="party-bank-warning" role="note">
        <p>
          <strong>{t('parties.bank_warning_title')}</strong>
        </p>
        <p>{t('parties.bank_warning_fraud')}</p>
        <p>{t('parties.bank_warning_audit')}</p>
        <p class="muted">{t('parties.bank_warning_human')}</p>
      </div>
      <FormProblem error={problem} />
      <form class="party-bank-form" onSubmit={(event) => event.preventDefault()}>
        <Field id="party-bank-iban" label={t('parties.iban')} value={iban} onInput={(v) => setIban(v)} onBlur={checkBank} error={errors.iban} mono ltr required />
        <Field id="party-bank-bic" label={t('parties.bic')} value={bic} onInput={(v) => setBic(v)} onBlur={checkBank} error={errors.bic} mono ltr hint={t('parties.bic_hint')} />
        <Field id="party-bank-reason" label={t('parties.bank_reason')} value={reason} onInput={(v) => setReason(v)} error={errors.reason} required hint={t('parties.bank_reason_hint')} />
      </form>
    </ConfirmDialog>
  );
}

export function MergeDialog({
  source,
  open,
  onClose,
  onMerged,
}: {
  source: PartyDetail;
  open: boolean;
  onClose: () => void;
  onMerged: (survivor: Party) => void;
}) {
  const { client } = useSession();
  const [survivor, setSurvivor] = useState<Party | null>(null);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<unknown>(null);
  const preview = mergePreview(source, survivor);

  const merge = async () => {
    if (!survivor) return;
    setBusy(true);
    setProblem(null);
    try {
      await partyMerge(client, survivor.id, source.party.id);
      onMerged(survivor);
    } catch (error) {
      setProblem(error);
    } finally {
      setBusy(false);
    }
  };

  const refusal =
    preview.reason === 'same'
      ? t('parties.merge_same')
      : preview.reason === 'merged'
        ? t('parties.merge_source_merged')
        : preview.reason === 'survivor-merged'
          ? t('parties.merge_survivor_merged')
          : null;

  return (
    <ConfirmDialog
      open={open}
      onClose={onClose}
      title={t('parties.merge_title')}
      confirmLabel={t('parties.merge_confirm')}
      onConfirm={merge}
      busy={busy}
      disabled={!preview.ok || busy}
      danger
      error={problem}
    >
      <p>{t('parties.merge_intro', { name: source.party.name })}</p>
      <PartyPicker id="merge-survivor" label={t('parties.merge_into')} selected={survivor} onPick={setSurvivor} exclude={source.party.id} />
      {survivor && (
        <section class="party-merge-preview" aria-labelledby="merge-preview-title">
          <h3 id="merge-preview-title">{t('parties.merge_preview_title')}</h3>
          {refusal && (
            <p class="banner error" role="alert">
              {refusal}
            </p>
          )}
          {preview.ok && (
            <ul class="party-merge-list">
              <li>{t('parties.merge_moves_bills', { n: preview.billsMove, name: survivor.name })}</li>
              <li>{preview.keepsVat ? t('parties.merge_keeps_vat') : t('parties.merge_no_vat')}</li>
              <li>{preview.keepsBank ? t('parties.merge_keeps_bank') : t('parties.merge_no_bank')}</li>
              {(preview.keepsAddresses > 0 || preview.keepsContacts > 0) && (
                <li>{t('parties.merge_keeps_details', { addresses: preview.keepsAddresses, contacts: preview.keepsContacts })}</li>
              )}
              <li>{t('parties.merge_bank_lines')}</li>
              <li>{t('parties.merge_read_only', { name: source.party.name })}</li>
            </ul>
          )}
          {preview.ok && (
            <p class="muted">
              <Ltr>{source.party.name}</Ltr> → <Ltr>{survivor.name}</Ltr>
            </p>
          )}
        </section>
      )}
    </ConfirmDialog>
  );
}
