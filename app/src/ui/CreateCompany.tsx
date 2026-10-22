/**
 * "Create company": a name, a country pack, a base currency and the first month
 * of the fiscal year. The pack sets the currency and the fiscal year to start
 * from, and the person may change both. On success the app switches to the new
 * company (the creator is its owner). The server checks the same rules; the
 * form checks them first so the person sees the field that is wrong.
 */
import { useEffect, useRef, useState } from 'preact/hooks';
import { companyCreate, countryPacksList, type CountryPackSummary } from '../api/companies';
import { useAsync } from '../hooks';
import { intlLocale, t, type MessageKey } from '../i18n';
import { companyHref, companyNameProblem, currencyProblem, fiscalYearStartProblem } from '../lib/companies';
import { resetScope, useSession } from '../session';
import { ErrorBanner, Skeleton } from './common';
import { FieldProblem, FormProblem } from './settings/common';
import { Select, type SelectOption } from './Select';

export function CreateCompanyDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
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
      class="confirm key-dialog create-company-dialog"
      aria-labelledby="create-company-title"
      onClose={onClose}
      onCancel={(event) => {
        event.preventDefault();
        onClose();
      }}
    >
      {open && <CreateCompanyForm onClose={onClose} />}
    </dialog>
  );
}

function monthName(month: number): string {
  return new Intl.DateTimeFormat(intlLocale(), { month: 'long', timeZone: 'UTC' }).format(new Date(Date.UTC(2026, month - 1, 1)));
}

type Field = 'name' | 'baseCurrency' | 'fiscalYearStartMonth';

const PROBLEM: Record<'empty' | 'too-long', MessageKey> = {
  empty: 'companies.name_required',
  'too-long': 'companies.name_too_long',
};

function CreateCompanyForm({ onClose }: { onClose: () => void }) {
  const { client, toast } = useSession();
  const packs = useAsync(() => countryPacksList(client), [client.baseUrl]);
  const [name, setName] = useState('');
  const [pack, setPack] = useState('');
  const [currency, setCurrency] = useState('');
  const [month, setMonth] = useState(1);
  // Until the person changes the currency or the month, the chosen pack sets them.
  const [touched, setTouched] = useState<{ currency: boolean; month: boolean }>({ currency: false, month: false });
  const [attempted, setAttempted] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);

  if (packs.error !== undefined) {
    return (
      <div class="dialog-body">
        <h2 id="create-company-title">{t('companies.create_title')}</h2>
        <ErrorBanner error={packs.error} onRetry={packs.reload} />
        <div class="dialog-actions">
          <button type="button" class="btn" onClick={onClose}>
            {t('companies.cancel')}
          </button>
        </div>
      </div>
    );
  }
  if (!packs.data) {
    return (
      <div class="dialog-body">
        <h2 id="create-company-title">{t('companies.create_title')}</h2>
        <Skeleton rows={4} label={t('companies.loading')} />
      </div>
    );
  }

  const list: readonly CountryPackSummary[] = packs.data;
  const selected = list.find((item) => item.id === (pack || list[0]?.id)) ?? null;
  const chosenPack = selected?.id ?? '';
  const chosenCurrency = touched.currency ? currency : (selected?.currency ?? currency);
  const chosenMonth = touched.month ? month : (selected?.fiscalYearStartMonth ?? month);
  const nameProblem = companyNameProblem(name);
  const problems: Partial<Record<Field, MessageKey>> = {};
  if (nameProblem) problems.name = PROBLEM[nameProblem];
  if (currencyProblem(chosenCurrency)) problems.baseCurrency = 'companies.currency_invalid';
  if (fiscalYearStartProblem(chosenMonth)) problems.fiscalYearStartMonth = 'companies.fiscal_start_invalid';
  const shown = (field: Field) => (attempted && problems[field] ? t(problems[field]!) : null);

  const options: SelectOption[] = list.map((item) => ({
    value: item.id,
    label: item.name,
    hint: item.iso,
  }));
  const monthOptions: SelectOption[] = Array.from({ length: 12 }, (_, index) => ({
    value: String(index + 1),
    label: monthName(index + 1),
  }));

  async function submit(event: Event) {
    event.preventDefault();
    setAttempted(true);
    setError(null);
    if (Object.keys(problems).length > 0) {
      const first = Object.keys(problems)[0];
      document.getElementById(`create-company-${first}`)?.focus();
      return;
    }
    setBusy(true);
    try {
      const created = await companyCreate(client, {
        name: name.trim(),
        baseCurrency: chosenCurrency,
        countryPack: chosenPack,
        fiscalYearStartMonth: chosenMonth,
      });
      toast(t('companies.created', { name: created.name }));
      // The new company starts with the defaults: no unit scope, this year to date.
      resetScope();
      window.location.assign(companyHref(window.location.pathname, created.id));
    } catch (err) {
      setError(err);
      setBusy(false);
    }
  }

  return (
    <form class="dialog-body create-company-form" onSubmit={submit} noValidate aria-labelledby="create-company-title">
      <h2 id="create-company-title">{t('companies.create_title')}</h2>
      <p class="hint">{t('companies.create_intro')}</p>
      <FormProblem error={error} />
      <label class="field">
        <span>{t('companies.name')}</span>
        <input
          id="create-company-name"
          class="input"
          name="name"
          value={name}
          maxLength={200}
          required
          autoComplete="organization"
          aria-invalid={shown('name') ? 'true' : undefined}
          onInput={(event) => setName((event.target as HTMLInputElement).value)}
        />
        <FieldProblem id="create-company-name-error" message={shown('name')} />
      </label>
      <div class="field-row">
        <label class="field">
          <span>{t('companies.country_pack')}</span>
          <Select
            id="create-company-pack"
            label={t('companies.country_pack')}
            value={chosenPack}
            options={options}
            searchable={false}
            onChange={(next) => {
              setPack(next);
              setTouched({ currency: false, month: false });
            }}
          />
        </label>
        <label class="field">
          <span>{t('companies.base_currency')}</span>
          <input
            id="create-company-baseCurrency"
            class="input mono"
            name="baseCurrency"
            value={chosenCurrency}
            maxLength={3}
            autoComplete="off"
            aria-invalid={shown('baseCurrency') ? 'true' : undefined}
            onInput={(event) => {
              setTouched((prev) => ({ ...prev, currency: true }));
              setCurrency((event.target as HTMLInputElement).value.toUpperCase());
            }}
          />
          <FieldProblem id="create-company-baseCurrency-error" message={shown('baseCurrency')} />
        </label>
      </div>
      <label class="field">
        <span>{t('companies.fiscal_start')}</span>
        <Select
          id="create-company-fiscalYearStartMonth"
          label={t('companies.fiscal_start')}
          value={String(chosenMonth)}
          options={monthOptions}
          searchable={false}
          onChange={(next) => {
            setTouched((prev) => ({ ...prev, month: true }));
            setMonth(Number(next));
          }}
        />
        <span class="hint">{t('companies.fiscal_start_hint')}</span>
      </label>
      <div class="dialog-actions">
        <button type="button" class="btn" onClick={onClose} disabled={busy}>
          {t('companies.cancel')}
        </button>
        <button type="submit" class="btn btn-primary" disabled={busy}>
          {t('companies.create_submit')}
        </button>
      </div>
    </form>
  );
}
