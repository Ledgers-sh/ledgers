import { accountsList, reportsTrialBalance } from '../api/commands';
import type { Account } from '../api/types';
import { useAsync } from '../hooks';
import { useSession } from '../session';
import { Empty, ErrorBanner, Money, PageHead, Skeleton } from './common';

/** Turns a translation key such as "account.cash_at_bank" into a readable fallback label. */
export function accountLabel(nameKey: string): string {
  const bare = nameKey.replace(/^account\./, '').replace(/[._-]+/g, ' ').trim();
  return bare.charAt(0).toUpperCase() + bare.slice(1);
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

  if (state.loading && !state.data) return <Skeleton rows={8} label="Loading accounts" />;
  if (state.error && !state.data) return <ErrorBanner error={state.error} onRetry={state.reload} />;
  if (!state.data) return null;

  const balances = new Map(state.data.trial.map((row) => [row.account_code, row.balance]));
  const accounts: Account[] = [...state.data.accounts].sort((a, b) => a.number.localeCompare(b.number));

  return (
    <>
      <PageHead title="Accounts" actions={<span class="muted">Balances for {period}</span>}>
        The chart of accounts with balances from the trial balance. Debit-positive; a credit balance shows as negative.
      </PageHead>
      {accounts.length === 0 ? (
        <Empty title="No accounts yet">
          Accounts are created with the CLI (<code>ledgers account create</code>) or the HTTP API. They appear here as soon as they exist.
        </Empty>
      ) : (
        <section class="panel" aria-label="Chart of accounts">
          <div class="table-wrap">
            <table class="ledger cards">
              <thead>
                <tr>
                  <th scope="col">Code</th>
                  <th scope="col">Account</th>
                  <th scope="col">Type</th>
                  <th scope="col" class="num">
                    Balance
                  </th>
                </tr>
              </thead>
              <tbody>
                {accounts.map((account) => {
                  const balance = balances.get(account.number);
                  return (
                    <tr key={account.number}>
                      <td class="code" data-label="Code">
                        {account.number}
                      </td>
                      <td class="main desc" data-label="Account">
                        {accountLabel(account.name_key)}
                        {!account.is_postable && <span class="muted"> (header)</span>}
                      </td>
                      <td data-label="Type" class="muted">
                        {account.account_type}
                      </td>
                      <td class="num" data-label="Balance">
                        {balance ? <Money minor={balance.minor} currency={balance.currency} /> : <span class="muted">—</span>}
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
