import { accountsList, reportsTrialBalance } from '../api/commands';
import type { Account } from '../api/types';
import { useAsync } from '../hooks';
import { useSession } from '../session';
import { accountName, formatPeriod, t, type MessageKey } from '../i18n';
import { Ltr } from '../i18n/Ltr';
import { Empty, ErrorBanner, Money, PageHead, Skeleton } from './common';

const TYPE_KEYS: Record<string, MessageKey> = {
  asset: 'accountType.asset',
  liability: 'accountType.liability',
  equity: 'accountType.equity',
  income: 'accountType.income',
  expense: 'accountType.expense',
};

/** The account's type, translated when the app knows it; the server's own word otherwise. */
function accountTypeLabel(accountType: string): string {
  const key = TYPE_KEYS[accountType];
  return key ? t(key) : accountType;
}

export function Accounts() {
  const { client, period } = useSession();
  const state = useAsync(
    async () => {
      const [accounts, trial] = await Promise.all([accountsList(client), reportsTrialBalance(client, period)]);
      return { accounts: accounts.accounts, trial: trial.rows };
    },
    [client.baseUrl, client.apiKey, period],
  );

  if (state.loading && !state.data) return <Skeleton rows={8} label={t('accounts.loading')} />;
  if (state.error && !state.data) return <ErrorBanner error={state.error} onRetry={state.reload} />;
  if (!state.data) return null;

  const balances = new Map(state.data.trial.map((row) => [row.account_code, row.balance]));
  const accounts: Account[] = [...state.data.accounts].sort((a, b) => a.number.localeCompare(b.number));

  return (
    <>
      <PageHead title={t('accounts.title')} actions={<span class="muted">{t('accounts.balances_for', { period: formatPeriod(period) })}</span>}>
        {t('accounts.intro')}
      </PageHead>
      {accounts.length === 0 ? (
        <Empty title={t('accounts.empty_title')}>{t('accounts.empty_body')}</Empty>
      ) : (
        <section class="panel" aria-label={t('accounts.section')}>
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
                {accounts.map((account) => {
                  const balance = balances.get(account.number);
                  return (
                    <tr key={account.number}>
                      <td class="code" data-label={t('term.code')}>
                        <Ltr>{account.number}</Ltr>
                      </td>
                      <td class="main desc" data-label={t('term.account')}>
                        {accountName(account.name_key)}
                        {!account.is_postable && <span class="muted"> {t('accounts.header_suffix')}</span>}
                      </td>
                      <td data-label={t('term.type')} class="muted">
                        {accountTypeLabel(account.account_type)}
                      </td>
                      <td class="num" data-label={t('term.balance')}>
                        {balance ? <Money minor={balance.minor} currency={balance.currency} /> : <span class="muted">{t('common.empty_cell')}</span>}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </section>
      )}
    </>
  );
}
