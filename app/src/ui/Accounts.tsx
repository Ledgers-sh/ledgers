/**
 * The chart of accounts: every account grouped by class (Assets to Expenses),
 * the parent/child tree indented, search and an active / archived / all filter,
 * balances in mono. Each name opens the account's detail page; "New account"
 * opens the form.
 */
import { useMemo, useState } from 'preact/hooks';
import { accountsList } from '../api/commands';
import type { Account } from '../api/types';
import { useAsync } from '../hooks';
import { useSession } from '../session';
import { accountName, t, useLocale, type MessageKey } from '../i18n';
import { Ltr } from '../i18n/Ltr';
import { accountLabel, buildChart, filterChart, nonZeroBalances, type StatusFilter, type TreeRow } from '../lib/accounts';
import { Empty, ErrorBanner, Money, PageHead, Skeleton } from './common';

const CLASS_KEYS: Record<string, MessageKey> = {
  asset: 'accounts.group.asset',
  liability: 'accounts.group.liability',
  equity: 'accounts.group.equity',
  income: 'accounts.group.income',
  expense: 'accounts.group.expense',
};

const TYPE_KEYS: Record<string, MessageKey> = {
  asset: 'accountType.asset',
  liability: 'accountType.liability',
  equity: 'accountType.equity',
  income: 'accountType.income',
  expense: 'accountType.expense',
};

const FILTERS: { value: StatusFilter; key: MessageKey }[] = [
  { value: 'active', key: 'accounts.filter.active' },
  { value: 'archived', key: 'accounts.filter.archived' },
  { value: 'all', key: 'accounts.filter.all' },
];

/** The label of an account in the language in use. */
export function labelOf(account: Pick<Account, 'name_key' | 'display_name'>): string {
  return accountLabel(account, accountName);
}

export function Accounts() {
  const { client, unit } = useSession();
  useLocale();
  const state = useAsync(async () => (await accountsList(client)).accounts, [client.baseUrl, client.apiKey]);
  const [query, setQuery] = useState('');
  const [status, setStatus] = useState<StatusFilter>('active');

  const groups = useMemo(() => {
    if (!state.data) return [];
    return filterChart(buildChart(state.data), { query, status, label: labelOf });
  }, [state.data, query, status]);

  if (state.loading && !state.data) return <Skeleton rows={8} label={t('accounts.loading')} />;
  if (state.error && !state.data) return <ErrorBanner error={state.error} onRetry={state.reload} />;
  if (!state.data) return null;

  const accounts = state.data;
  const newButton = (
    <span class="row-actions">
      <a class="btn" href="#/accounts/units">
        {t('units.nav')}
      </a>
      <a class="btn btn-primary" href="#/accounts/new">
        {t('accounts.new')}
      </a>
    </span>
  );

  return (
    <>
      <PageHead title={t('accounts.title')} actions={newButton}>
        {t('accounts.intro')}
      </PageHead>
      {unit !== null && <p class="note" role="note">{t('units.accounts_scope_note', { code: unit })}</p>}
      {state.error && <ErrorBanner error={state.error} onRetry={state.reload} />}
      {accounts.length === 0 ? (
        <Empty title={t('accounts.empty_title')} action={<div class="row-actions">{newButton}</div>}>
          {t('accounts.empty_body')}
        </Empty>
      ) : (
        <>
          <div class="panel-head">
            <label class="field grow">
              <span class="sr-only">{t('accounts.search_label')}</span>
              <input
                class="input"
                type="search"
                placeholder={t('accounts.search_placeholder')}
                aria-label={t('accounts.search_label')}
                value={query}
                onInput={(event) => setQuery((event.target as HTMLInputElement).value)}
              />
            </label>
            <div class="tabs" role="tablist" aria-label={t('accounts.filter_label')}>
              {FILTERS.map((filter) => (
                <button
                  key={filter.value}
                  type="button"
                  role="tab"
                  id={`accounts-filter-${filter.value}`}
                  class="tab"
                  aria-selected={status === filter.value ? 'true' : 'false'}
                  aria-controls="accounts-chart"
                  onClick={() => setStatus(filter.value)}
                >
                  {t(filter.key)}
                </button>
              ))}
            </div>
          </div>
          <div id="accounts-chart" role="tabpanel" aria-labelledby={`accounts-filter-${status}`}>
            {groups.length === 0 ? (
              <Empty title={t('accounts.no_matches_title')}>{t('accounts.no_matches_body')}</Empty>
            ) : (
              groups.map((group) => (
                <section key={group.type} aria-labelledby={`group-${group.type}`}>
                  <h2 id={`group-${group.type}`}>{t(CLASS_KEYS[group.type])}</h2>
                  <div class="table-wrap">
                    <table class="ledger cards">
                      <thead>
                        <tr>
                          <th scope="col">{t('term.code')}</th>
                          <th scope="col">{t('term.account')}</th>
                          <th scope="col">{t('term.type')}</th>
                          <th scope="col" class="num">
                            {t('term.balance')}
                          </th>
                        </tr>
                      </thead>
                      <tbody>
                        {group.rows.map((row) => (
                          <AccountRow key={row.account.number} row={row} />
                        ))}
                      </tbody>
                    </table>
                  </div>
                </section>
              ))
            )}
          </div>
        </>
      )}
    </>
  );
}

function AccountRow({ row }: { row: TreeRow }) {
  const { account, depth, hasChildren } = row;
  const balances = nonZeroBalances(account);
  return (
    <tr>
      <td class="code" data-label={t('term.code')}>
        <Ltr>{account.number}</Ltr>
      </td>
      <td class="main desc" data-label={t('term.account')}>
        <div style={{ paddingInlineStart: `${depth * 1.5}rem` }}>
          {depth > 0 && <span class="muted" aria-hidden="true">└ </span>}
          <a class="row-link" href={`#/accounts/${encodeURIComponent(account.number)}`}>
            {labelOf(account)}
          </a>{' '}
          {hasChildren && <span class="badge">{t('accounts.heading_tag')}</span>}{' '}
          {account.archived && <span class="badge">{t('accounts.status.archived')}</span>}
        </div>
      </td>
      <td data-label={t('term.type')} class="muted">
        {t(TYPE_KEYS[account.account_type] ?? 'accountType.asset')}
      </td>
      <td class="num balance-cell" data-label={t('term.balance')}>
        {balances.length === 0 ? (
          <span class="muted">{t('common.empty_cell')}</span>
        ) : (
          balances.map((balance) => <Money key={balance.currency} minor={balance.minor} currency={balance.currency} />)
        )}
      </td>
    </tr>
  );
}
