import { useMemo, useState } from 'preact/hooks';
import { unitSelectOption } from './UnitOption';
import { entriesPost, accountsList } from '../api/commands';
import { ApiError } from '../api/client';
import { checkBalance } from '../lib/balance';
import { buildPostEntryInput, toCheckLines, type DraftLine } from '../lib/entryDraft';
import { formatMinor, parseMajorToMinor } from '../lib/money';
import { todayIso, yearOf } from '../lib/period';
import { useAsync } from '../hooks';
import { loadCurrency, saveCurrency, useSession } from '../session';
import { accountNeedsUnit } from '../lib/units';
import { Empty, ErrorBanner, PageHead, Skeleton } from './common';
import { accountName, intlLocale, t, tRich, type MessageKey } from '../i18n';
import { Ltr } from '../i18n/Ltr';
import { entryRef } from './Journal';
import { Select } from './Select';

const DEFAULT_JOURNAL = 'general';

function newLine(side: 'debit' | 'credit', unit: string): DraftLine {
  return { account: '', side, amount: '', memo: '', unit };
}

export function NewEntry() {
  const { client, period, currency: sessionCurrency, setCurrency, online, toast, unit: scopeUnit, units } = useSession();
  const unitsEnabled = units.length > 0;
  const accounts = useAsync(async () => (await accountsList(client)).accounts, [client.baseUrl, client.apiKey]);
  const postable = useMemo(
    () => (accounts.data ?? []).filter((account) => account.is_postable && !account.archived).sort((a, b) => a.number.localeCompare(b.number)),
    [accounts.data],
  );

  const [journalId, setJournalId] = useState(DEFAULT_JOURNAL);
  const [fiscalYear, setFiscalYear] = useState(() => yearOf(period));
  const [date, setDate] = useState(todayIso());
  const [description, setDescription] = useState('');
  const [currency, setCurrencyState] = useState(sessionCurrency || loadCurrency());
  // New lines take the unit the switcher shows, so a unit's entries are posted from its own screen.
  const [lines, setLines] = useState<DraftLine[]>(() => [newLine('debit', scopeUnit ?? ''), newLine('credit', scopeUnit ?? '')]);
  const [touched, setTouched] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  // One key per form instance: a retried submit reuses it, so the server can de-duplicate.
  const [externalId] = useState(() => `app-${crypto.randomUUID()}`);
  const [serverError, setServerError] = useState<unknown>(null);

  const currencyValid = /^[A-Z]{3}$/.test(currency);
  const minors = lines.map((line) => parseMajorToMinor(line.amount, currencyValid ? currency : 'XXX'));
  const balance = checkBalance(toCheckLines(lines, minors.map((m, i) => (lines[i].amount.trim() === '' ? 0n : m))));
  const lineErrors = lines.map((line, i) => {
    if (!touched) return null;
    if (line.account === '') return t('new.error_account');
    if (line.amount.trim() === '') return t('new.error_amount_missing');
    if (minors[i] === null) return t('new.error_amount_format');
    if (minors[i] === 0n) return t('new.error_amount_zero');
    return null;
  });
  // Once the company has units, income, expense and inter-unit lines must name one. The check runs on every
  // render so a submit is never sent without it; the message shows once the person has tried to post.
  const missingUnit = lines.map(
    (line) => unitsEnabled && line.unit === '' && accountNeedsUnit(postable.find((account) => account.number === line.account)),
  );
  const unitErrors = lines.map((_, i) => (touched && missingUnit[i] ? t('units.err_line') : null));
  const descriptionError = touched && description.trim() === '' ? t('new.error_description') : null;
  const currencyError = touched && !currencyValid ? t('new.error_currency') : null;
  const fyError = touched && (!Number.isInteger(fiscalYear) || fiscalYear < 1900) ? t('new.error_fiscal_year') : null;

  const ready =
    balance.balanced &&
    descriptionError === null &&
    currencyError === null &&
    fyError === null &&
    lineErrors.every((error) => error === null) &&
    !missingUnit.some(Boolean) &&
    journalId.trim() !== '';
  const difference = balance.difference;
  const offBy = difference < 0n ? -difference : difference;

  const update = (index: number, patch: Partial<DraftLine>) =>
    setLines((prev) => prev.map((line, i) => (i === index ? { ...line, ...patch } : line)));

  const submit = async (event: Event) => {
    event.preventDefault();
    setTouched(true);
    setServerError(null);
    if (!ready || !online) return;
    const input = buildPostEntryInput(
      { journalId: journalId.trim(), fiscalYear, date, description: description.trim(), currency, externalId },
      lines,
      minors as bigint[],
    );
    setSubmitting(true);
    try {
      const result = await entriesPost(client, input);
      saveCurrency(currency);
      setCurrency(currency);
      toast(t('new.posted', { ref: entryRef(result.entry) }));
      window.location.hash = '#/journal';
    } catch (error) {
      setServerError(error);
    } finally {
      setSubmitting(false);
    }
  };

  if (accounts.loading && !accounts.data) return <Skeleton rows={5} label={t('new.loading')} />;
  if (accounts.error && !accounts.data) return <ErrorBanner error={accounts.error} onRetry={accounts.reload} />;
  const locale = intlLocale();
  const amountCurrency = currencyValid ? currency : 'XXX';

  return (
    <>
      <PageHead title={t('new.title')} actions={<a class="btn btn-ghost" href="#/journal">{t('new.cancel')}</a>}>
        {t('new.intro')}
      </PageHead>

      {postable.length === 0 ? (
        <Empty
          title={t('new.no_accounts_title')}
          action={
            <a class="btn" href="#/accounts">
              {t('new.view_accounts')}
            </a>
          }
        >
          {t('new.no_accounts_body')}
        </Empty>
      ) : (
        <form class="panel" onSubmit={submit} noValidate aria-label={t('new.form_label')}>
          <div class="panel-body entry-form">
            <div class="header-grid">
              <label class="field">
                <span>{t('new.date')}</span>
                <input class="input" type="date" value={date} onInput={(e) => setDate((e.target as HTMLInputElement).value)} required />
              </label>
              <label class="field wide">
                <span>{t('new.description')}</span>
                <input
                  class="input"
                  type="text"
                  name="description"
                  maxLength={200}
                  placeholder={t('new.description_placeholder')}
                  value={description}
                  aria-invalid={descriptionError ? 'true' : undefined}
                  aria-describedby={descriptionError ? 'desc-error' : undefined}
                  onInput={(e) => setDescription((e.target as HTMLInputElement).value)}
                />
                {descriptionError && <span id="desc-error" class="field-error">{descriptionError}</span>}
              </label>
              <label class="field">
                <span>{t('new.currency')}</span>
                <input
                  class="input mono"
                  type="text"
                  name="currency"
                  maxLength={3}
                  value={currency}
                  aria-invalid={currencyError ? 'true' : undefined}
                  aria-describedby={currencyError ? 'cur-error' : 'cur-hint'}
                  onInput={(e) => setCurrencyState((e.target as HTMLInputElement).value.toUpperCase())}
                />
                {currencyError ? (
                  <span id="cur-error" class="field-error">{currencyError}</span>
                ) : (
                  <span id="cur-hint" class="hint">
                    {t('new.currency_hint')}
                  </span>
                )}
              </label>
              <label class="field">
                <span>{t('new.journal')}</span>
                <input class="input mono" type="text" value={journalId} onInput={(e) => setJournalId((e.target as HTMLInputElement).value)} />
              </label>
              <label class="field">
                <span>{t('new.fiscal_year')}</span>
                <input
                  class="input mono"
                  type="number"
                  min={1900}
                  max={9999}
                  value={fiscalYear}
                  aria-invalid={fyError ? 'true' : undefined}
                  onInput={(e) => setFiscalYear(Number((e.target as HTMLInputElement).value))}
                />
                {fyError && <span class="field-error">{fyError}</span>}
              </label>
            </div>

            <fieldset class="lines" style={{ border: 0, padding: 0, margin: 0 }}>
              <legend class="sr-only">{t('new.lines')}</legend>
              <div class={`line-head${unitsEnabled ? ' with-units' : ''}`} aria-hidden="true">
                <span>{t('new.line_account')}</span>
                <span>{t('new.col_side')}</span>
                <span>{t('term.amount')}</span>
                {unitsEnabled && <span>{t('units.field')}</span>}
                <span>{t('new.col_memo')}</span>
                <span />
              </div>
              {lines.map((line, index) => (
                <div class={`line-row${unitsEnabled ? ' with-units' : ''}`} key={index}>
                  <div class="field account">
                    <span class="label-sm">{t('new.line_account')}</span>
                    <Select
                      id={`line-account-${index}`}
                      label={t('new.line_account_aria', { n: index + 1 })}
                      value={line.account}
                      placeholder={t('new.choose_account')}
                      invalid={touched && line.account === ''}
                      options={postable.map((account) => ({
                        value: account.number,
                        label: accountName(account.name_key),
                        hint: account.number,
                        group: t(`accountType.${account.account_type}` as MessageKey),
                      }))}
                      onChange={(value) => update(index, { account: value })}
                    />
                    {lineErrors[index] && line.account === '' && <span class="field-error">{lineErrors[index]}</span>}
                  </div>
                  <div class="field side">
                    <span class="label-sm">{t('new.col_side')}</span>
                    <Select
                      id={`line-side-${index}`}
                      label={t('new.line_side_aria', { n: index + 1 })}
                      value={line.side}
                      searchable={false}
                      options={[
                        { value: 'debit', label: t('new.side_debit') },
                        { value: 'credit', label: t('new.side_credit') },
                      ]}
                      onChange={(value) => update(index, { side: value as 'debit' | 'credit' })}
                    />
                  </div>
                  <label class="field amount">
                    <span class="label-sm">{t('term.amount')}</span>
                    <input
                      class="input num"
                      inputMode="decimal"
                      autoComplete="off"
                      placeholder="0.00"
                      value={line.amount}
                      aria-label={t('new.line_amount_aria', { n: index + 1 })}
                      aria-invalid={lineErrors[index] && line.amount !== '' ? 'true' : undefined}
                      onInput={(e) => update(index, { amount: (e.target as HTMLInputElement).value })}
                    />
                    {lineErrors[index] && line.account !== '' && <span class="field-error">{lineErrors[index]}</span>}
                  </label>
                  {unitsEnabled && (
                    <div class="field unit">
                      <span class="label-sm">{t('units.field')}</span>
                      <Select
                        id={`line-unit-${index}`}
                        label={t('units.line_unit_aria', { n: index + 1 })}
                        value={line.unit}
                        placeholder={t('units.choose_unit')}
                        invalid={unitErrors[index] !== null}
                        options={units.map((item) => (unitSelectOption(item)))}
                        searchable={false}
                        onChange={(value) => update(index, { unit: value })}
                      />
                      {unitErrors[index] && <span class="field-error">{unitErrors[index]}</span>}
                    </div>
                  )}
                  <label class="field memo">
                    <span class="label-sm">{t('new.col_memo')}</span>
                    <input
                      class="input"
                      value={line.memo}
                      maxLength={200}
                      aria-label={t('new.line_memo_aria', { n: index + 1 })}
                      onInput={(e) => update(index, { memo: (e.target as HTMLInputElement).value })}
                    />
                  </label>
                  <button
                    type="button"
                    class="btn btn-ghost btn-sm remove"
                    aria-label={t('new.remove_line', { n: index + 1 })}
                    disabled={lines.length <= 2}
                    onClick={() => setLines((prev) => prev.filter((_, i) => i !== index))}
                  >
                    ×
                  </button>
                </div>
              ))}
              <div>
                <button type="button" class="btn btn-sm" onClick={() => setLines((prev) => [...prev, newLine('debit', scopeUnit ?? '')])}>
                  {t('new.add_line')}
                </button>
              </div>
            </fieldset>

            <section class={`balance ${balance.balanced ? 'ok' : touched || balance.filled > 0 ? 'off' : ''}`} aria-live="polite" aria-label={t('new.balance')}>
              <div class="cell">
                <span class="label">{t('new.debits')}</span>
                <span class="value"><Ltr>{formatMinor(balance.debit, amountCurrency, locale)}</Ltr></span>
              </div>
              <div class="cell">
                <span class="label">{t('new.credits')}</span>
                <span class="value"><Ltr>{formatMinor(balance.credit, amountCurrency, locale)}</Ltr></span>
              </div>
              <div class="cell">
                <span class="label">{t('new.difference')}</span>
                <span class="value diff">
                  {balance.filled < 2 && difference === 0n
                    ? t('new.enter_both')
                    : difference === 0n
                    ? t('new.balanced')
                    : tRich(difference < 0n ? 'new.credits_exceed' : 'new.debits_exceed', {
                        amount: <Ltr>{`${formatMinor(offBy, amountCurrency, locale)}${currencyValid ? ` ${currency}` : ''}`}</Ltr>,
                      })}
                </span>
              </div>
            </section>

            {balance.hasInvalid && touched && <p class="field-error">{t('new.fix_amounts')}</p>}

            {serverError !== null && <ErrorBanner error={serverError} />}
            {serverError instanceof ApiError && serverError.kind === 'forbidden' && (
              <p class="note">{t('new.key_cannot_post')}</p>
            )}

            <div class="form-actions">
              {!online && <span class="muted grow">{t('common.offline_posting')}</span>}
              {touched && !balance.balanced && !balance.hasInvalid && (
                <span class="muted grow" role="status">
                  {t('new.make_equal')}
                </span>
              )}
              <button type="submit" class="btn btn-primary" disabled={submitting || !online} aria-busy={submitting ? 'true' : undefined}>
                {submitting ? t('new.posting') : t('new.post')}
              </button>
            </div>
            <p class="hint muted" style={{ fontSize: '13px' }}>
              {t('new.retry_safe')}
            </p>
          </div>
        </form>
      )}
    </>
  );
}
