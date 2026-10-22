/**
 * One account: its balance and class, rename and tags, archive or restore
 * (with the rule spelled out), and its recent entries, each linking to the
 * journal entry it came from.
 */
import { useEffect, useState } from 'preact/hooks';
import { accountsArchive, accountsGet, accountsRestore, accountsUpdate } from '../api/commands';
import { ApiError } from '../api/client';
import type { Account, AccountDetailOutput } from '../api/types';
import { useAsync } from '../hooks';
import { useSession } from '../session';
import { formatDate, intlLocale, t, useLocale, type MessageKey } from '../i18n';
import { Ltr } from '../i18n/Ltr';
import { hasPostings, MAX_NAME_LENGTH, nonZeroBalances, parseTags, tagErrors } from '../lib/accounts';
import { formatMinor } from '../lib/money';
import { ConfirmDialog, Empty, ErrorBanner, Money, PageHead, Skeleton } from './common';
import { labelOf } from './Accounts';

const TYPE_KEYS: Record<string, MessageKey> = {
  asset: 'accountType.asset',
  liability: 'accountType.liability',
  equity: 'accountType.equity',
  income: 'accountType.income',
  expense: 'accountType.expense',
};

const TAG_ERROR_KEYS: Record<string, MessageKey> = {
  format: 'accounts.err.tags_format',
  duplicate: 'accounts.err.tags_duplicate',
  too_many: 'accounts.err.tags_too_many',
};

export function AccountDetail({ number }: { number: string }) {
  const { client, toast } = useSession();
  useLocale();
  const state = useAsync(() => accountsGet(client, number), [client.baseUrl, client.apiKey, number]);
  const [name, setName] = useState('');
  const [tags, setTags] = useState('');
  const [nameSubmitted, setNameSubmitted] = useState(false);
  const [busy, setBusy] = useState<'name' | 'tags' | 'status' | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [confirming, setConfirming] = useState(false);

  const account = state.data?.account;

  // Refill the edit fields whenever the account changes on the server.
  useEffect(() => {
    if (!account) return;
    // A template account has no custom name yet: start from the name it shows, so a rename edits it.
    setName(labelOf(account));
    setTags(account.tags.join(', '));
    setError(null);
  }, [account?.number, account?.display_name, account?.name_key, account?.tags.join(',')]);

  if (state.loading && !state.data) return <Skeleton rows={6} label={t('accounts.loading_one')} />;
  if (state.error && !state.data) {
    const notFound = state.error instanceof ApiError && state.error.status === 404;
    return (
      <>
        <BackLink />
        {notFound ? (
          <Empty title={t('accounts.not_found')} />
        ) : (
          <ErrorBanner error={state.error} onRetry={state.reload} />
        )}
      </>
    );
  }
  if (!state.data || !account) return null;

  const detail: AccountDetailOutput = state.data;
  const balances = nonZeroBalances(account);
  const label = labelOf(account);
  const nameError = name.trim() === '' ? t('accounts.err.required') : name.trim().length > MAX_NAME_LENGTH ? t('accounts.err.name_too_long') : null;
  const tagCode = tagErrors(tags);
  const tagError = tagCode ? t(TAG_ERROR_KEYS[tagCode]) : null;

  const saveName = async (event: Event) => {
    event.preventDefault();
    setNameSubmitted(true);
    if (nameError || busy) return;
    await write('name', () => accountsUpdate(client, account.number, { name: name.trim() }), t('accounts.saved'));
  };
  const saveTags = async (event: Event) => {
    event.preventDefault();
    if (tagError || busy) return;
    await write('tags', () => accountsUpdate(client, account.number, { tags: parseTags(tags) }), t('accounts.saved'));
  };
  const changeStatus = async () => {
    const restoring = account.archived;
    await write(
      'status',
      () => (restoring ? accountsRestore(client, account.number) : accountsArchive(client, account.number)),
      restoring ? t('accounts.restored_toast', { number: account.number }) : t('accounts.archived_toast', { number: account.number }),
      () => setConfirming(false),
    );
  };

  async function write(kind: 'name' | 'tags' | 'status', call: () => Promise<unknown>, message: string, done?: () => void) {
    setBusy(kind);
    setError(null);
    try {
      await call();
      toast(message);
      done?.();
      state.reload();
    } catch (err) {
      setError(err);
    } finally {
      setBusy(null);
    }
  }

  return (
    <>
      <BackLink />
      <PageHead
        title={label}
        actions={
          <span class="row-actions">
            <span class="badge">{t(TYPE_KEYS[account.account_type] ?? 'accountType.asset')}</span>
            <span class={`badge ${account.archived ? '' : 'active'}`}>
              {account.archived ? t('accounts.status.archived') : t('accounts.status.active')}
            </span>
          </span>
        }
      >
        <Ltr>{account.number}</Ltr>
      </PageHead>

      {account.archived && (
        <div class="banner human" role="status">
          <span class="grow">{t('accounts.archived_notice')}</span>
        </div>
      )}

      <section class="panel" aria-label={t('accounts.detail.balance')}>
          <dl class="entry-meta detail-meta">
            <div>
              <dt>{t('accounts.detail.balance')}</dt>
              <dd class="amount-value">
                {balances.length === 0 ? (
                  <span class="muted">{t('accounts.detail.balance_none')}</span>
                ) : (
                  balances.map((balance) => <Money key={balance.currency} minor={balance.minor} currency={balance.currency} />)
                )}
              </dd>
            </div>
            <div>
              <dt>{t('accounts.detail.type')}</dt>
              <dd>{t(TYPE_KEYS[account.account_type] ?? 'accountType.asset')}</dd>
            </div>
            <div>
              <dt>{t('accounts.detail.normal')}</dt>
              <dd>{t(account.normal_balance === 'credit' ? 'term.credit' : 'term.debit')}</dd>
            </div>
            <div>
              <dt>{t('accounts.detail.kind')}</dt>
              <dd>{account.is_postable ? t('accounts.kind_postable') : t('accounts.kind_heading')}</dd>
            </div>
            <div>
              <dt>{t('accounts.detail.parent')}</dt>
              <dd>
                {account.parent ? (
                  <a href={`#/accounts/${encodeURIComponent(account.parent)}`}>
                    <Ltr>{account.parent}</Ltr>
                  </a>
                ) : (
                  t('accounts.detail.parent_none')
                )}
              </dd>
            </div>
            <div>
              <dt>{t('new.currency')}</dt>
              <dd>{account.currency ? <Ltr>{account.currency}</Ltr> : t('common.empty_cell')}</dd>
            </div>
          </dl>
      </section>

      <section class="panel" aria-labelledby="account-rename-title">
        <div class="panel-head">
          <h2 id="account-rename-title">{t('accounts.rename_title')}</h2>
        </div>
        <form class="panel-body stack" onSubmit={saveName} noValidate>
              <label class="field">
                <span class="sr-only">{t('accounts.rename_title')}</span>
                <input
                  id="account-name"
                  class="input"
                  type="text"
                  value={name}
                  maxLength={MAX_NAME_LENGTH + 20}
                  aria-invalid={nameSubmitted && nameError ? 'true' : undefined}
                  aria-describedby={nameSubmitted && nameError ? 'account-name-error' : undefined}
                  onInput={(event) => setName((event.target as HTMLInputElement).value)}
                />
                {nameSubmitted && nameError && (
                  <span id="account-name-error" class="field-error">
                    {nameError}
                  </span>
                )}
              </label>
              <div class="form-actions">
                <button type="submit" class="btn btn-primary" disabled={busy !== null} aria-busy={busy === 'name' ? 'true' : undefined}>
                  {busy === 'name' ? t('common.working') : t('accounts.rename_save')}
                </button>
              </div>
        </form>
      </section>

      <section class="panel" aria-labelledby="account-tags-title">
        <div class="panel-head">
          <h2 id="account-tags-title">{t('accounts.tags_title')}</h2>
        </div>
        <form class="panel-body stack" onSubmit={saveTags} noValidate>
              <label class="field">
                <span class="sr-only">{t('accounts.tags_title')}</span>
                <input
                  id="account-tags"
                  class="input mono"
                  type="text"
                  value={tags}
                  aria-invalid={tagError ? 'true' : undefined}
                  aria-describedby={tagError ? 'account-tags-error' : 'account-tags-hint'}
                  onInput={(event) => setTags((event.target as HTMLInputElement).value)}
                />
                {tagError ? (
                  <span id="account-tags-error" class="field-error">
                    {tagError}
                  </span>
                ) : (
                  <span id="account-tags-hint" class="hint">
                    {t('accounts.form.tags_hint')}
                  </span>
                )}
              </label>
              <div class="form-actions">
                <button type="submit" class="btn btn-primary" disabled={busy !== null || tagError !== null} aria-busy={busy === 'tags' ? 'true' : undefined}>
                  {busy === 'tags' ? t('common.working') : t('accounts.tags_save')}
                </button>
              </div>
        </form>
      </section>

      <section class="panel" aria-labelledby="account-status-title">
        <div class="panel-head">
          <h2 id="account-status-title">{account.archived ? t('accounts.restore') : t('accounts.archive')}</h2>
        </div>
        <div class="panel-body stack">
          <p class="muted">{account.archived ? t('accounts.restore_body') : t('accounts.archive_body')}</p>
          {error !== null && <ErrorBanner error={error} />}
          <div class="form-actions">
            <button type="button" class={`btn ${account.archived ? '' : 'btn-danger'}`} onClick={() => setConfirming(true)} disabled={busy !== null}>
              {account.archived ? t('accounts.restore') : t('accounts.archive')}
            </button>
          </div>
        </div>
      </section>

      <section class="panel" aria-labelledby="account-activity-title">
        <div class="panel-head">
          <h2 id="account-activity-title">{t('accounts.activity_title')}</h2>
        </div>
        {detail.activity.length === 0 ? (
          <div class="panel-body">
            <p class="muted">{t('accounts.activity_empty')}</p>
          </div>
        ) : (
          <div class="table-wrap">
            <table class="ledger cards">
              <thead>
                <tr>
                  <th scope="col">{t('term.posting_date')}</th>
                  <th scope="col">{t('term.description')}</th>
                  <th scope="col">{t('term.entry')}</th>
                  <th scope="col" class="num">
                    {t('term.amount')}
                  </th>
                </tr>
              </thead>
              <tbody>
                {detail.activity.map((line) => (
                  <tr key={line.entry_id}>
                    <td data-label={t('term.posting_date')} class="muted">
                      <Ltr>{formatDate(line.posting_date)}</Ltr>
                    </td>
                    <td class="main desc" data-label={t('term.description')}>
                      {line.description}
                    </td>
                    <td data-label={t('term.entry')}>
                      <a class="row-link" href={`#/journal/${encodeURIComponent(line.entry_id)}`}>
                        {t('accounts.activity_entry', { number: `${line.journal_id}-${line.fiscal_year}-${String(line.number).padStart(4, '0')}` })}
                      </a>
                    </td>
                    <td class="num" data-label={t('term.amount')}>
                      <Money minor={line.minor} currency={line.currency} />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>

      <ConfirmDialog
        open={confirming}
        onClose={() => setConfirming(false)}
        title={account.archived ? t('accounts.restore_title') : t('accounts.archive_title')}
        confirmLabel={account.archived ? t('accounts.restore_confirm') : t('accounts.archive_confirm')}
        onConfirm={() => void changeStatus()}
        busy={busy === 'status'}
        danger={!account.archived}
        disabled={!account.archived && hasPostings(account)}
        error={error}
      >
        {account.archived ? (
          <p>{t('accounts.restore_body')}</p>
        ) : (
          <>
            <p>{t('accounts.archive_body')}</p>
            {hasPostings(account) && <p class="field-error">{blockedText(account)}</p>}
          </>
        )}
      </ConfirmDialog>
    </>
  );
}

/** The refusal sentence for an archive: the balance that keeps the account open. */
function blockedText(account: Account): string {
  const first = nonZeroBalances(account)[0];
  const amount = first ? formatMinor(first.minor, first.currency, intlLocale()) : '';
  return t('accounts.archive_blocked', { balance: amount });
}

function BackLink() {
  return (
    <a class="back-link" href="#/accounts">
      {t('accounts.back')}
    </a>
  );
}
