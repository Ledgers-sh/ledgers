import { useMemo, useState } from 'preact/hooks';
import { entriesPost, accountsList } from '../api/commands';
import { ApiError } from '../api/client';
import { checkBalance } from '../lib/balance';
import { buildPostEntryInput, toCheckLines, type DraftLine } from '../lib/entryDraft';
import { formatMinor, parseMajorToMinor } from '../lib/money';
import { todayIso, yearOf } from '../lib/period';
import { useAsync } from '../hooks';
import { loadCurrency, saveCurrency, useSession } from '../session';
import { Empty, ErrorBanner, LOCALE, PageHead, Skeleton } from './common';
import { accountLabel } from './Accounts';
import { entryRef } from './Journal';

const DEFAULT_JOURNAL = 'general';

function newLine(side: 'debit' | 'credit'): DraftLine {
  return { account: '', side, amount: '', memo: '' };
}

export function NewEntry() {
  const { client, period, currency: sessionCurrency, setCurrency, online, toast } = useSession();
  const accounts = useAsync(async () => (await accountsList(client)).accounts, [client.baseUrl, client.apiKey]);
  const postable = useMemo(
    () => (accounts.data ?? []).filter((account) => account.is_postable).sort((a, b) => a.number.localeCompare(b.number)),
    [accounts.data],
  );

  const [journalId, setJournalId] = useState(DEFAULT_JOURNAL);
  const [fiscalYear, setFiscalYear] = useState(() => yearOf(period));
  const [date, setDate] = useState(todayIso());
  const [description, setDescription] = useState('');
  const [currency, setCurrencyState] = useState(sessionCurrency || loadCurrency());
  const [lines, setLines] = useState<DraftLine[]>([newLine('debit'), newLine('credit')]);
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
    if (line.account === '') return 'Choose an account.';
    if (line.amount.trim() === '') return 'Enter an amount.';
    if (minors[i] === null) return 'Use digits with an optional decimal point.';
    if (minors[i] === 0n) return 'Amount must be more than zero.';
    return null;
  });
  const descriptionError = touched && description.trim() === '' ? 'Describe what this entry records.' : null;
  const currencyError = touched && !currencyValid ? 'Use a three-letter code, such as EUR.' : null;
  const fyError = touched && (!Number.isInteger(fiscalYear) || fiscalYear < 1900) ? 'Enter a fiscal year.' : null;

  const ready =
    balance.balanced &&
    descriptionError === null &&
    currencyError === null &&
    fyError === null &&
    lineErrors.every((error) => error === null) &&
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
      toast(`Posted ${entryRef(result.entry)}.`);
      window.location.hash = '#/journal';
    } catch (error) {
      setServerError(error);
    } finally {
      setSubmitting(false);
    }
  };

  if (accounts.loading && !accounts.data) return <Skeleton rows={5} label="Loading accounts" />;
  if (accounts.error && !accounts.data) return <ErrorBanner error={accounts.error} onRetry={accounts.reload} />;

  return (
    <>
      <PageHead title="New entry" actions={<a class="btn btn-ghost" href="#/journal">Cancel</a>}>
        A balanced double-entry posting. The server checks it again before anything is written.
      </PageHead>

      {postable.length === 0 ? (
        <Empty
          title="No accounts to post to yet"
          action={
            <a class="btn" href="#/accounts">
              View accounts
            </a>
          }
        >
          An entry needs at least two postable accounts. Create them with the CLI or the API first.
        </Empty>
      ) : (
        <form class="panel" onSubmit={submit} noValidate aria-label="New journal entry">
          <div class="panel-body entry-form">
            <div class="header-grid">
              <label class="field">
                <span>Posting date</span>
                <input class="input" type="date" value={date} onInput={(e) => setDate((e.target as HTMLInputElement).value)} required />
              </label>
              <label class="field wide">
                <span>Description</span>
                <input
                  class="input"
                  type="text"
                  name="description"
                  maxLength={200}
                  placeholder="What this entry records"
                  value={description}
                  aria-invalid={descriptionError ? 'true' : undefined}
                  aria-describedby={descriptionError ? 'desc-error' : undefined}
                  onInput={(e) => setDescription((e.target as HTMLInputElement).value)}
                />
                {descriptionError && <span id="desc-error" class="field-error">{descriptionError}</span>}
              </label>
              <label class="field">
                <span>Currency</span>
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
                    One currency per entry. Amounts use its decimals.
                  </span>
                )}
              </label>
              <label class="field">
                <span>Journal</span>
                <input class="input mono" type="text" value={journalId} onInput={(e) => setJournalId((e.target as HTMLInputElement).value)} />
              </label>
              <label class="field">
                <span>Fiscal year</span>
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
              <legend class="sr-only">Lines</legend>
              <div class="line-head" aria-hidden="true">
                <span>Account</span>
                <span>Side</span>
                <span>Amount</span>
                <span>Memo</span>
                <span />
              </div>
              {lines.map((line, index) => (
                <div class="line-row" key={index}>
                  <label class="field account">
                    <span class="label-sm">Account</span>
                    <select
                      class="input"
                      value={line.account}
                      aria-label={`Line ${index + 1} account`}
                      aria-invalid={touched && line.account === '' ? 'true' : undefined}
                      onChange={(e) => update(index, { account: (e.target as HTMLSelectElement).value })}
                    >
                      <option value="">Choose account</option>
                      {postable.map((account) => (
                        <option key={account.number} value={account.number}>
                          {account.number} · {accountLabel(account.name_key)}
                        </option>
                      ))}
                    </select>
                    {lineErrors[index] && line.account === '' && <span class="field-error">{lineErrors[index]}</span>}
                  </label>
                  <label class="field side">
                    <span class="label-sm">Side</span>
                    <select
                      class="input"
                      value={line.side}
                      aria-label={`Line ${index + 1} side`}
                      onChange={(e) => update(index, { side: (e.target as HTMLSelectElement).value as 'debit' | 'credit' })}
                    >
                      <option value="debit">Debit</option>
                      <option value="credit">Credit</option>
                    </select>
                  </label>
                  <label class="field amount">
                    <span class="label-sm">Amount</span>
                    <input
                      class="input num"
                      inputMode="decimal"
                      autoComplete="off"
                      placeholder="0.00"
                      value={line.amount}
                      aria-label={`Line ${index + 1} amount`}
                      aria-invalid={lineErrors[index] && line.amount !== '' ? 'true' : undefined}
                      onInput={(e) => update(index, { amount: (e.target as HTMLInputElement).value })}
                    />
                    {lineErrors[index] && line.account !== '' && <span class="field-error">{lineErrors[index]}</span>}
                  </label>
                  <label class="field memo">
                    <span class="label-sm">Memo</span>
                    <input
                      class="input"
                      value={line.memo}
                      maxLength={200}
                      aria-label={`Line ${index + 1} memo`}
                      onInput={(e) => update(index, { memo: (e.target as HTMLInputElement).value })}
                    />
                  </label>
                  <button
                    type="button"
                    class="btn btn-ghost btn-sm remove"
                    aria-label={`Remove line ${index + 1}`}
                    disabled={lines.length <= 2}
                    onClick={() => setLines((prev) => prev.filter((_, i) => i !== index))}
                  >
                    ×
                  </button>
                </div>
              ))}
              <div>
                <button type="button" class="btn btn-sm" onClick={() => setLines((prev) => [...prev, newLine('debit')])}>
                  Add line
                </button>
              </div>
            </fieldset>

            <section class={`balance ${balance.balanced ? 'ok' : touched || balance.filled > 0 ? 'off' : ''}`} aria-live="polite" aria-label="Balance">
              <div class="cell">
                <span class="label">Debits</span>
                <span class="value">{formatMinor(balance.debit, currencyValid ? currency : 'XXX', LOCALE)}</span>
              </div>
              <div class="cell">
                <span class="label">Credits</span>
                <span class="value">{formatMinor(balance.credit, currencyValid ? currency : 'XXX', LOCALE)}</span>
              </div>
              <div class="cell">
                <span class="label">Difference</span>
                <span class="value diff">
                  {balance.filled < 2 && difference === 0n
                    ? 'Enter both sides'
                    : difference === 0n
                    ? 'Balanced'
                    : `${difference < 0n ? 'Credits' : 'Debits'} exceed by ${formatMinor(offBy, currencyValid ? currency : 'XXX', LOCALE)} ${currencyValid ? currency : ''}`.trim()}
                </span>
              </div>
            </section>

            {balance.hasInvalid && touched && <p class="field-error">Fix the amounts marked above before posting.</p>}

            {serverError !== null && <ErrorBanner error={serverError} />}
            {serverError instanceof ApiError && serverError.kind === 'forbidden' && (
              <p class="note">This key cannot post. Ask the owner for a key with posting rights.</p>
            )}

            <div class="form-actions">
              {!online && <span class="muted grow">offline: posting needs the server</span>}
              {touched && !balance.balanced && !balance.hasInvalid && (
                <span class="muted grow" role="status">
                  Make debits equal credits to post.
                </span>
              )}
              <button type="submit" class="btn btn-primary" disabled={submitting || !online} aria-busy={submitting ? 'true' : undefined}>
                {submitting ? 'Posting…' : 'Post entry'}
              </button>
            </div>
            <p class="hint muted" style={{ fontSize: '13px' }}>
              Retrying a failed post is safe: this form keeps one reference for the entry, so the server does not post it twice.
            </p>
          </div>
        </form>
      )}
    </>
  );
}
