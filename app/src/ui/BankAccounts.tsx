/** The Bank screens' front: the accounts with their balances, and the page to open one. */
import { bankAccountsList, type AccountView } from '../api/bank';
import { useAsync } from '../hooks';
import { useSession } from '../session';
import { formatIban } from '../lib/bank';
import { t } from '../i18n';
import { Empty, ErrorBanner, Money, PageHead, Skeleton } from './common';
import { BankAccountForm } from './BankAccountForm';
import { BankTabs, DifferenceValue } from './BankShared';
import { Icon } from './icons';

export function BankAccounts() {
  const { client } = useSession();
  const person = !client.apiKey;
  const accounts = useAsync(() => bankAccountsList(client).then((r) => r.accounts), [client.baseUrl, client.apiKey]);
  const list = accounts.data ?? [];

  return (
    <>
      <PageHead
        title={t('bank.title')}
        actions={
          <>
            <a class="btn" href="#/bank/rules">
              {t('bank.tab_rules')}
            </a>
            {person && list.length > 0 && (
              <a class="btn btn-primary" href="#/bank/new">
                <Icon name="plus" />
                {t('bank.new_account')}
              </a>
            )}
          </>
        }
      >
        {t('bank.intro')}
      </PageHead>
      <BankTabs current="accounts" />

      {accounts.loading && accounts.data === undefined && <Skeleton rows={3} label={t('bank.loading')} />}
      {accounts.error !== undefined && accounts.error !== null && <ErrorBanner error={accounts.error} onRetry={accounts.reload} />}

      {accounts.data !== undefined && list.length === 0 && (
        <>
          <Empty title={t('bank.empty_title')}>{person ? t('bank.empty_body') : t('bank.empty_agent')}</Empty>
          {person && (
            <section class="panel bank-new-panel" aria-labelledby="bank-new-title">
              <div class="panel-head">
                <h2 id="bank-new-title">{t('bank.new_account')}</h2>
              </div>
              <div class="panel-body">
                <BankAccountForm />
              </div>
            </section>
          )}
        </>
      )}

      {list.length > 0 && (
        <ul class="bank-accounts" aria-label={t('bank.accounts_list')}>
          {list.map((account) => (
            <AccountCard key={account.id} account={account} />
          ))}
        </ul>
      )}

      <p class="note bank-footer">{t('bank.feeds_note')}</p>
    </>
  );
}

function AccountCard({ account }: { account: AccountView }) {
  return (
    <li>
      <a class="bank-card" href={`#/bank/${encodeURIComponent(account.id)}`}>
        <span class="bank-card-head">
          <strong class="bank-card-name">{account.name}</strong>
          <span class="chip">{account.currency}</span>
        </span>
        <span class="bank-iban mono">{formatIban(account.iban)}</span>
        <dl class="bank-balances">
          <div>
            <dt>{t('bank.statement_balance')}</dt>
            <dd>
              {account.statement_balance ? (
                <Money minor={account.statement_balance.amount.minor} currency={account.statement_balance.amount.currency} />
              ) : (
                <span class="muted">{t('bank.no_statement')}</span>
              )}
            </dd>
          </div>
          <div>
            <dt>{t('bank.ledger_balance')}</dt>
            <dd>
              <Money minor={account.ledger_balance.minor} currency={account.ledger_balance.currency} />
            </dd>
          </div>
          <div>
            <dt>{t('bank.difference')}</dt>
            <dd>
              <DifferenceValue minor={account.difference.minor} currency={account.difference.currency} />
            </dd>
          </div>
        </dl>
        <span class="bank-card-counts muted">
          {t('bank.counts', { unmatched: account.unmatched_count, matched: account.matched_count, ignored: account.ignored_count })}
        </span>
      </a>
    </li>
  );
}

export function BankAccountNew() {
  const { client } = useSession();
  return (
    <>
      <PageHead title={t('bank.new_account')}>{t('bank.new_intro')}</PageHead>
      <BankTabs current="accounts" />
      {client.apiKey ? (
        <p class="note">{t('bank.empty_agent')}</p>
      ) : (
        <section class="panel" aria-labelledby="bank-new-page-title">
          <div class="panel-head">
            <h2 id="bank-new-page-title">{t('bank.new_account')}</h2>
          </div>
          <div class="panel-body">
            <BankAccountForm />
          </div>
        </section>
      )}
    </>
  );
}
