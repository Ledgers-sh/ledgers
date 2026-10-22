/**
 * The new-account form. Each field is checked inline as the person leaves it
 * (and again on submit) against the loaded chart; the server checks the same
 * rules and its refusal is shown when it disagrees.
 */
import { useMemo, useState } from 'preact/hooks';
import { accountsCreate, accountsList } from '../api/commands';
import { ApiError } from '../api/client';
import { useAsync } from '../hooks';
import { useSession } from '../session';
import { t, useLocale, type MessageKey } from '../i18n';
import { ACCOUNT_TYPES, hasErrors, parentOptions, parseTags, validateDraft, type DraftErrors, type DraftInput } from '../lib/accounts';
import { ErrorBanner, Skeleton } from './common';
import { labelOf } from './Accounts';
import './accounts.css';

type Field = keyof DraftInput;

const FIELD_ORDER: Field[] = ['number', 'name', 'type', 'parent', 'tags', 'currency'];
const EMPTY: DraftInput = { number: '', name: '', type: '', parent: '', tags: '', currency: '' };

/** The message key of each validation code, per field. */
const ERROR_KEYS: Partial<Record<Field, Record<string, MessageKey>>> = {
  number: { required: 'accounts.err.required', format: 'accounts.err.number_format', taken: 'accounts.err.number_taken' },
  name: { required: 'accounts.err.required', too_long: 'accounts.err.name_too_long' },
  type: { required: 'accounts.err.required' },
  parent: { same_type: 'accounts.err.parent_same_type', has_postings: 'accounts.err.parent_has_postings' },
  tags: { format: 'accounts.err.tags_format', duplicate: 'accounts.err.tags_duplicate', too_many: 'accounts.err.tags_too_many' },
  currency: { format: 'accounts.err.currency_format' },
};

function messageFor(field: Field, errors: DraftErrors): string | null {
  const code = errors[field];
  const key = code ? ERROR_KEYS[field]?.[code] : undefined;
  return key ? t(key) : null;
}

export function AccountNew() {
  const { client, toast } = useSession();
  useLocale();
  const list = useAsync(async () => (await accountsList(client)).accounts, [client.baseUrl, client.apiKey]);
  const accounts = list.data ?? [];

  const [form, setForm] = useState<DraftInput>(EMPTY);
  const [touched, setTouched] = useState<Partial<Record<Field, boolean>>>({});
  const [submitted, setSubmitted] = useState(false);
  const [busy, setBusy] = useState(false);
  const [serverError, setServerError] = useState<unknown>(null);
  const [serverNumberTaken, setServerNumberTaken] = useState(false);

  const errors = useMemo(() => validateDraft(form, accounts), [form, accounts]);
  const parents = useMemo(() => parentOptions(accounts, form.type), [accounts, form.type]);

  if (list.loading && !list.data) return <Skeleton rows={6} label={t('accounts.loading')} />;
  if (list.error && !list.data) return <ErrorBanner error={list.error} onRetry={list.reload} />;

  const shown = (field: Field): string | null => {
    if (field === 'number' && serverNumberTaken) return t('accounts.err.exists_server');
    return touched[field] || submitted ? messageFor(field, errors) : null;
  };
  const update = (field: Field, value: string) => {
    setForm((prev) => ({ ...prev, [field]: value }));
    if (field === 'number') setServerNumberTaken(false);
  };
  const touch = (field: Field) => setTouched((prev) => ({ ...prev, [field]: true }));

  const submit = async (event: Event) => {
    event.preventDefault();
    setSubmitted(true);
    setServerError(null);
    if (busy) return;
    if (hasErrors(errors)) {
      const first = FIELD_ORDER.find((field) => errors[field]);
      if (first) document.getElementById(`acct-${first}`)?.focus();
      return;
    }
    setBusy(true);
    const number = form.number.trim();
    try {
      await accountsCreate(client, {
        number,
        name: form.name.trim(),
        type: form.type,
        ...(form.parent !== '' ? { parent: form.parent } : {}),
        tags: parseTags(form.tags),
        ...(form.currency.trim() !== '' ? { currency: form.currency.trim() } : {}),
      });
      toast(t('accounts.created', { number }));
      window.location.hash = `#/accounts/${encodeURIComponent(number)}`;
    } catch (error) {
      if (error instanceof ApiError && (error.slug === 'account-exists' || error.status === 409)) {
        setServerNumberTaken(true);
        setSubmitted(true);
        document.getElementById('acct-number')?.focus();
      } else {
        setServerError(error);
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <a class="back-link" href="#/accounts">
        <span class="flip" aria-hidden="true">
          ←
        </span>{' '}
        {t('accounts.back')}
      </a>
      <header class="page-head">
        <div>
          <h1>{t('accounts.new_title')}</h1>
          <p>{t('accounts.new_intro')}</p>
        </div>
      </header>
      <form class="panel account-form" onSubmit={submit} noValidate aria-busy={busy ? 'true' : undefined}>
        <label class="field">
          <span>{t('accounts.form.number')}</span>
          <input
            id="acct-number"
            class="input mono"
            type="text"
            inputMode="numeric"
            autoComplete="off"
            maxLength={10}
            value={form.number}
            aria-invalid={shown('number') ? 'true' : undefined}
            aria-describedby={shown('number') ? 'acct-number-error' : 'acct-number-hint'}
            onInput={(event) => update('number', (event.target as HTMLInputElement).value)}
            onBlur={() => touch('number')}
          />
          {shown('number') ? (
            <span id="acct-number-error" class="field-error">
              {shown('number')}
            </span>
          ) : (
            <span id="acct-number-hint" class="hint">
              {t('accounts.form.number_hint')}
            </span>
          )}
        </label>

        <label class="field">
          <span>{t('accounts.form.name')}</span>
          <input
            id="acct-name"
            class="input"
            type="text"
            autoComplete="off"
            value={form.name}
            aria-invalid={shown('name') ? 'true' : undefined}
            aria-describedby={shown('name') ? 'acct-name-error' : undefined}
            onInput={(event) => update('name', (event.target as HTMLInputElement).value)}
            onBlur={() => touch('name')}
          />
          {shown('name') && (
            <span id="acct-name-error" class="field-error">
              {shown('name')}
            </span>
          )}
        </label>

        <label class="field">
          <span>{t('accounts.form.type')}</span>
          <select
            id="acct-type"
            class="input"
            value={form.type}
            aria-invalid={shown('type') ? 'true' : undefined}
            aria-describedby={shown('type') ? 'acct-type-error' : undefined}
            onChange={(event) => update('type', (event.target as HTMLSelectElement).value)}
            onBlur={() => touch('type')}
          >
            <option value="">{t('accounts.form.type')}</option>
            {ACCOUNT_TYPES.map((type) => (
              <option key={type} value={type}>
                {t(`accountType.${type}`)}
              </option>
            ))}
          </select>
          {shown('type') && (
            <span id="acct-type-error" class="field-error">
              {shown('type')}
            </span>
          )}
        </label>

        <label class="field">
          <span>{t('accounts.form.parent')}</span>
          <select
            id="acct-parent"
            class="input"
            value={form.parent}
            disabled={form.type === ''}
            aria-invalid={shown('parent') ? 'true' : undefined}
            aria-describedby={shown('parent') ? 'acct-parent-error' : undefined}
            onChange={(event) => update('parent', (event.target as HTMLSelectElement).value)}
            onBlur={() => touch('parent')}
          >
            <option value="">{t('accounts.form.parent_none')}</option>
            {parents.map((account) => (
              <option key={account.number} value={account.number}>
                {account.number} {labelOf(account)}
              </option>
            ))}
            {/* A parent picked for another class stays selectable, so its error can name it. */}
            {form.parent !== '' && !parents.some((account) => account.number === form.parent) && (
              <option value={form.parent}>{form.parent}</option>
            )}
          </select>
          {shown('parent') && (
            <span id="acct-parent-error" class="field-error">
              {shown('parent')}
            </span>
          )}
        </label>

        <label class="field wide">
          <span>{t('accounts.form.tags')}</span>
          <input
            id="acct-tags"
            class="input mono"
            type="text"
            autoComplete="off"
            value={form.tags}
            aria-invalid={shown('tags') ? 'true' : undefined}
            aria-describedby={shown('tags') ? 'acct-tags-error' : 'acct-tags-hint'}
            onInput={(event) => update('tags', (event.target as HTMLInputElement).value)}
            onBlur={() => touch('tags')}
          />
          {shown('tags') ? (
            <span id="acct-tags-error" class="field-error">
              {shown('tags')}
            </span>
          ) : (
            <span id="acct-tags-hint" class="hint">
              {t('accounts.form.tags_hint')}
            </span>
          )}
        </label>

        <label class="field">
          <span>{t('accounts.form.currency')}</span>
          <input
            id="acct-currency"
            class="input mono"
            type="text"
            autoComplete="off"
            maxLength={3}
            value={form.currency}
            aria-invalid={shown('currency') ? 'true' : undefined}
            aria-describedby={shown('currency') ? 'acct-currency-error' : 'acct-currency-hint'}
            onInput={(event) => update('currency', (event.target as HTMLInputElement).value.toUpperCase())}
            onBlur={() => touch('currency')}
          />
          {shown('currency') ? (
            <span id="acct-currency-error" class="field-error">
              {shown('currency')}
            </span>
          ) : (
            <span id="acct-currency-hint" class="hint">
              {t('accounts.form.currency_hint')}
            </span>
          )}
        </label>

        {serverError !== null && (
          <div class="wide">
            <ErrorBanner error={serverError} />
          </div>
        )}

        <div class="form-actions wide">
          <a class="btn" href="#/accounts">
            {t('common.cancel')}
          </a>
          <button type="submit" class="btn btn-primary" disabled={busy} aria-busy={busy ? 'true' : undefined}>
            {busy ? t('common.working') : t('accounts.form.submit')}
          </button>
        </div>
      </form>
    </>
  );
}
