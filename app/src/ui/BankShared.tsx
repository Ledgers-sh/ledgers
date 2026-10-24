/** Pieces the bank screens share: the tabs, the chart the pickers read, amounts and status chips. */
import { accountsList } from '../api/commands';
import type { Account } from '../api/types';
import { useAsync, type AsyncState } from '../hooks';
import { useSession } from '../session';
import { accountLabel } from '../lib/accounts';
import { signedAmount, type LineStatus } from '../lib/bank';
import { accountName, intlLocale, t, type MessageKey } from '../i18n';
import { Ltr } from '../i18n/Ltr';
import type { SelectOption } from './Select';
import { TabRow } from './TabRow';

export function BankTabs({ current }: { current: 'accounts' | 'rules' }) {
  return (
    <TabRow active={current} label={t('bank.sections')} class="bank-tabs">
      <a class="tab" href="#/bank" aria-current={current === 'accounts' ? 'page' : undefined}>
        {t('bank.tab_accounts')}
      </a>
      <a class="tab" href="#/bank/rules" aria-current={current === 'rules' ? 'page' : undefined}>
        {t('bank.tab_rules')}
      </a>
    </TabRow>
  );
}

/** The whole chart, for the pickers and the rule and journal screens. */
export function useChart(): AsyncState<Account[]> {
  const { client } = useSession();
  return useAsync(() => accountsList(client).then((r) => r.accounts), [client.baseUrl, client.apiKey]);
}

export function chartLabel(account: Account): string {
  return accountLabel(account, accountName);
}

/** Chart accounts tagged `bank`: the ledger accounts a bank account can link to. */
export function bankLedgerAccounts(chart: Account[]): Account[] {
  return chart.filter((account) => account.is_postable && !account.archived && account.tags.includes('bank'));
}

/** Accounts an entry can post its other side to: postable, not archived, not a bank account. */
export function counterpartAccounts(chart: Account[]): Account[] {
  return chart.filter((account) => account.is_postable && !account.archived && !account.tags.includes('bank'));
}

export function accountOptions(accounts: Account[]): SelectOption[] {
  return accounts.map((account) => ({ value: account.number, label: chartLabel(account), hint: account.number }));
}

/** A signed amount for lists: + for money in, a minus sign for money out, in the reader's language. */
export function SignedAmount({ minor, currency, class: className }: { minor: string; currency: string; class?: string }) {
  const { text, sign } = signedAmount(minor, currency, intlLocale());
  const label = sign === 'in' ? t('bank.amount_in') : sign === 'out' ? t('bank.amount_out') : '';
  return (
    <span class={`num bank-amount bank-amount-${sign} ${className ?? ''}`.trim()}>
      <Ltr>{text}</Ltr>
      <span class="sr-only">
        {' '}
        {currency}
        {label ? `, ${label}` : ''}
      </span>
    </span>
  );
}

const STATUS_KEYS: Record<LineStatus, MessageKey> = {
  unmatched: 'bank.status_unmatched',
  suggested: 'bank.status_suggested',
  matched: 'bank.status_matched',
  ignored: 'bank.status_ignored',
};

export function StatusChip({ status }: { status: LineStatus }) {
  return <span class={`chip bank-status bank-status-${status}`}>{t(STATUS_KEYS[status])}</span>;
}

/** The statement's balance against the ledger's: a difference is the warning colour; a match is the neutral one. */
export function DifferenceValue({ minor, currency }: { minor: string; currency: string }) {
  const balanced = BigInt(minor) === 0n;
  return (
    <span class={`bank-diff ${balanced ? 'ok' : 'warn'}`}>
      <SignedAmount minor={minor} currency={currency} />
      <span class="bank-diff-note">{balanced ? t('bank.balanced') : t('bank.not_balanced')}</span>
    </span>
  );
}
