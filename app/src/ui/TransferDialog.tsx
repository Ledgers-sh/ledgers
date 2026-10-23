/**
 * "Transfer between units": recovers a shared cost from the unit that paid it.
 * The form checks what the server would refuse (two different units, a
 * positive amount, an income account to recover and an expense account to
 * book), shows what each unit's books will receive, then posts entries.transfer.
 */
import { useEffect, useRef, useState } from 'preact/hooks';
import { accountsList, entriesTransfer } from '../api/commands';
import { companyGet } from '../api/settings';
import type { Account, Unit } from '../api/types';
import { ApiError } from '../api/client';
import { useAsync } from '../hooks';
import { useSession } from '../session';
import { accountName, formatDate, intlLocale, t, type MessageKey } from '../i18n';
import { Ltr } from '../i18n/Ltr';
import { entryRef } from './Journal';
import { formatMinor, parseMajorToMinor } from '../lib/money';
import { todayIso } from '../lib/period';
import { buildTransferInput, validateTransfer, type TransferDraft, type TransferField } from '../lib/transfer';
import { unitSelectOption } from './UnitOption';
import { colourOf } from '../lib/units';
import { ErrorBanner, Skeleton } from './common';
import { Select, type SelectOption } from './Select';

export function TransferDialog({
  open,
  onClose,
  units,
  onPosted,
}: {
  open: boolean;
  onClose: () => void;
  units: readonly Unit[];
  onPosted?: () => void;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const dialog = ref.current;
    if (!dialog) return;
    if (open && !dialog.open) dialog.showModal();
    if (!open && dialog.open) dialog.close();
  }, [open]);
  return (
    <dialog
      ref={ref}
      class="confirm key-dialog transfer-dialog"
      aria-labelledby="transfer-title"
      onClose={onClose}
      onCancel={(event) => {
        event.preventDefault();
        onClose();
      }}
    >
      {open && <TransferForm units={units} onClose={onClose} onPosted={onPosted} />}
    </dialog>
  );
}

function emptyDraft(unitCode: string | null): TransferDraft {
  const today = todayIso();
  return {
    fromUnit: unitCode ?? '',
    toUnit: '',
    amount: '',
    recoveryAccount: '',
    costAccount: '',
    description: '',
    documentDate: today,
    postingDate: today,
  };
}

function TransferForm({ units, onClose, onPosted }: { units: readonly Unit[]; onClose: () => void; onPosted?: () => void }) {
  const { client, unit, toast } = useSession();
  const company = useAsync(() => companyGet(client), [client.baseUrl, client.apiKey]);
  const accounts = useAsync(async () => (await accountsList(client)).accounts, [client.baseUrl, client.apiKey]);
  const [draft, setDraft] = useState<TransferDraft>(() => emptyDraft(unit));
  const [touched, setTouched] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);

  if (company.error !== undefined || accounts.error !== undefined) {
    return (
      <div class="dialog-body">
        <h2 id="transfer-title">{t('units.transfer_title')}</h2>
        <ErrorBanner error={company.error ?? accounts.error} onRetry={accounts.reload} />
        <div class="dialog-actions">
          <button type="button" class="btn" onClick={onClose}>
            {t('units.cancel')}
          </button>
        </div>
      </div>
    );
  }
  if (!company.data || !accounts.data) {
    return (
      <div class="dialog-body">
        <h2 id="transfer-title">{t('units.transfer_title')}</h2>
        <Skeleton rows={4} label={t('units.transfer_loading')} />
      </div>
    );
  }

  const currency = company.data.base_currency;
  const chart: readonly Account[] = accounts.data;
  const postable = chart.filter((account) => account.is_postable && !account.archived).sort((a, b) => a.number.localeCompare(b.number));
  const recoveryOptions = postable.filter((account) => account.account_type === 'income');
  const costOptions = postable.filter((account) => account.account_type === 'expense');
  const problems = validateTransfer(draft, currency, chart);
  const shown = (field: TransferField) => (touched ? problems[field] : undefined);
  const messageFor = (field: TransferField) => {
    const key = shown(field);
    return key ? t(key as MessageKey) : null;
  };
  const update = (patch: Partial<TransferDraft>) => setDraft((prev) => ({ ...prev, ...patch }));
  const unitName = (code: string) => units.find((item) => item.code === code)?.name ?? code;
  const locale = intlLocale();
  const amountMinor = draft.amount.trim() === '' ? null : parseMajorToMinor(draft.amount, currency);
  const amountText = amountMinor === null ? '' : formatMinor(amountMinor, currency, locale);
  const accountLabel = (number: string) => {
    const found = chart.find((account) => account.number === number);
    return found ? `${number} ${accountName(found.name_key)}` : number;
  };

  const unitOptions: SelectOption[] = units.map((item) => ({
    value: item.code,
    label: item.code,
    hint: item.name,
    leading: unitSelectOption(item).leading,
  }));
  const accountOptions = (list: Account[]): SelectOption[] =>
    list.map((account) => ({ value: account.number, label: accountName(account.name_key), hint: account.number }));

  const submit = async (event: Event) => {
    event.preventDefault();
    setTouched(true);
    setError(null);
    if (Object.keys(problems).length > 0 || amountMinor === null) return;
    setBusy(true);
    try {
      const result = await entriesTransfer(client, buildTransferInput(draft, currency));
      toast(t('units.posted', { ref: entryRef(result.entry) }));
      onPosted?.();
      onClose();
    } catch (caught) {
      setError(caught);
    } finally {
      setBusy(false);
    }
  };

  const fieldError = (field: TransferField) => {
    const message = messageFor(field);
    return message ? <span class="field-error" id={`transfer-${field}-error`}>{message}</span> : null;
  };
  const invalid = (field: TransferField) => (shown(field) ? 'true' : undefined);

  return (
    <form class="dialog-body" onSubmit={submit} noValidate aria-labelledby="transfer-title">
      <h2 id="transfer-title">{t('units.transfer_title')}</h2>
      <p class="muted">{t('units.transfer_intro')}</p>

      {units.length < 2 ? (
        <div class="banner human" role="status">
          {t('units.transfer_needs_two')}
        </div>
      ) : (
        <>
          <div class="field-row">
            <div class="field">
              <span id="transfer-from-label">{t('units.from')}</span>
              <Select
                id="transfer-from"
                labelledBy="transfer-from-label"
                value={draft.fromUnit}
                placeholder={t('units.choose_unit')}
                invalid={shown('fromUnit') !== undefined}
                options={unitOptions}
                searchable={false}
                onChange={(value) => update({ fromUnit: value })}
              />
              {fieldError('fromUnit')}
            </div>
            <div class="field">
              <span id="transfer-to-label">{t('units.to')}</span>
              <Select
                id="transfer-to"
                labelledBy="transfer-to-label"
                value={draft.toUnit}
                placeholder={t('units.choose_unit')}
                invalid={shown('toUnit') !== undefined}
                options={unitOptions}
                searchable={false}
                onChange={(value) => update({ toUnit: value })}
              />
              {fieldError('toUnit')}
            </div>
          </div>

          <div class="field">
            <label for="transfer-amount">{t('units.amount', { currency })}</label>
            <input
              id="transfer-amount"
              class="input num"
              inputMode="decimal"
              autoComplete="off"
              placeholder="0.00"
              value={draft.amount}
              aria-invalid={invalid('amount')}
              aria-describedby={shown('amount') ? 'transfer-amount-error' : undefined}
              onInput={(event) => update({ amount: (event.target as HTMLInputElement).value })}
            />
            {fieldError('amount')}
          </div>

          <div class="field-row">
            <div class="field">
              <span id="transfer-recovery-label">{t('units.recovery_account')}</span>
              <Select
                id="transfer-recovery"
                labelledBy="transfer-recovery-label"
                value={draft.recoveryAccount}
                placeholder={t('units.choose_account')}
                invalid={shown('recoveryAccount') !== undefined}
                options={accountOptions(recoveryOptions)}
                onChange={(value) => update({ recoveryAccount: value })}
              />
              <span class="hint">{t('units.recovery_hint')}</span>
              {fieldError('recoveryAccount')}
            </div>
            <div class="field">
              <span id="transfer-cost-label">{t('units.cost_account')}</span>
              <Select
                id="transfer-cost"
                labelledBy="transfer-cost-label"
                value={draft.costAccount}
                placeholder={t('units.choose_account')}
                invalid={shown('costAccount') !== undefined}
                options={accountOptions(costOptions)}
                onChange={(value) => update({ costAccount: value })}
              />
              <span class="hint">{t('units.cost_hint')}</span>
              {fieldError('costAccount')}
            </div>
          </div>

          <div class="field">
            <label for="transfer-description">{t('units.description')}</label>
            <input
              id="transfer-description"
              class="input"
              maxLength={200}
              value={draft.description}
              aria-invalid={invalid('description')}
              aria-describedby={shown('description') ? 'transfer-description-error' : undefined}
              onInput={(event) => update({ description: (event.target as HTMLInputElement).value })}
            />
            {fieldError('description')}
          </div>

          <div class="field-row">
            <label class="field">
              <span>{t('units.document_date')}</span>
              <input class="input" type="date" value={draft.documentDate} onInput={(event) => update({ documentDate: (event.target as HTMLInputElement).value })} />
            </label>
            <label class="field">
              <span>{t('units.posting_date')}</span>
              <input class="input" type="date" value={draft.postingDate} onInput={(event) => update({ postingDate: (event.target as HTMLInputElement).value })} />
            </label>
          </div>
          {fieldError('documentDate')}

          <section class="transfer-preview" aria-label={t('units.preview')}>
            <h3>{t('units.preview')}</h3>
            <ul>
              <li>
                <span class="unit-dot" style={{ background: colourOf(units.find((item) => item.code === draft.fromUnit)) }} aria-hidden="true" />
                <span>
                  {draft.fromUnit ? unitName(draft.fromUnit) : t('units.choose_unit')}
                </span>
                <span class="muted">
                  {draft.recoveryAccount ? t('units.preview_income', { account: accountLabel(draft.recoveryAccount) }) : t('units.preview_income_none')}
                </span>
                <span class="num">{amountText ? <Ltr>{`+ ${amountText}`}</Ltr> : '–'}</span>
              </li>
              <li>
                <span class="unit-dot" style={{ background: colourOf(units.find((item) => item.code === draft.toUnit)) }} aria-hidden="true" />
                <span>{draft.toUnit ? unitName(draft.toUnit) : t('units.choose_unit')}</span>
                <span class="muted">
                  {draft.costAccount ? t('units.preview_expense', { account: accountLabel(draft.costAccount) }) : t('units.preview_expense_none')}
                </span>
                <span class="num">{amountText ? <Ltr>{`+ ${amountText}`}</Ltr> : '–'}</span>
              </li>
            </ul>
            <p class="hint">{t('units.preview_note', { date: formatDate(draft.postingDate) })}</p>
          </section>
        </>
      )}

      {error !== null && <ErrorBanner error={error} />}
      {error instanceof ApiError && error.kind === 'forbidden' && <p class="note">{t('new.key_cannot_post')}</p>}

      <div class="dialog-actions">
        <button type="button" class="btn" onClick={onClose} disabled={busy}>
          {t('units.cancel')}
        </button>
        <button type="submit" class="btn btn-primary" disabled={busy || units.length < 2} aria-busy={busy ? 'true' : undefined}>
          {busy ? t('new.posting') : t('units.transfer_submit')}
        </button>
      </div>
    </form>
  );
}
