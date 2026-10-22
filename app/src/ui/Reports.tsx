import { useState } from 'preact/hooks';
import { reportsBalanceSheet, reportsProfitAndLoss, reportsTrialBalance } from '../api/commands';
import { useAsync } from '../hooks';
import { useSession } from '../session';
import { periodEnd } from '../lib/period';
import { formatMinor, toPlainMajor } from '../lib/money';
import { toCsv } from '../lib/csv';
import { ErrorBanner, LOCALE, Money, PageHead, Skeleton, Empty } from './common';

type ReportKind = 'trial' | 'pl' | 'bs';
const TABS: { id: ReportKind; label: string }[] = [
  { id: 'trial', label: 'Trial balance' },
  { id: 'pl', label: 'Profit and loss' },
  { id: 'bs', label: 'Balance sheet' },
];

export function Reports() {
  const { client, period } = useSession();
  const [kind, setKind] = useState<ReportKind>('trial');
  const state = useAsync(
    async () => {
      if (kind === 'trial') return { kind, trial: await reportsTrialBalance(client, period) } as const;
      if (kind === 'pl') return { kind, pl: await reportsProfitAndLoss(client, period) } as const;
      return { kind, bs: await reportsBalanceSheet(client, periodEnd(period)) } as const;
    },
    [client.baseUrl, client.apiKey, period, kind],
  );

  const tabs = (
    <div role="tablist" aria-label="Report" class="row-actions" style={{ gap: '4px' }}>
      {TABS.map((tab) => (
        <button
          key={tab.id}
          type="button"
          role="tab"
          id={`tab-${tab.id}`}
          aria-selected={kind === tab.id}
          aria-controls="report-panel"
          class={`btn btn-sm ${kind === tab.id ? 'btn-primary' : 'btn-ghost'}`}
          onClick={() => setKind(tab.id)}
        >
          {tab.label}
        </button>
      ))}
    </div>
  );

  let body;
  let csv: { name: string; text: string } | null = null;
  if (state.loading && !state.data) body = <Skeleton rows={6} label="Loading report" />;
  else if (state.error && !state.data) body = <ErrorBanner error={state.error} onRetry={state.reload} />;
  else if (state.data?.kind === 'trial') {
    const { rows, period: p } = state.data.trial;
    if (rows.length === 0) {
      body = (
        <Empty title={`No activity in ${p}`}>
          The trial balance lists every account with postings in the period. Post an entry or choose another period.
        </Empty>
      );
    } else {
      const header = ['Account', 'Currency', 'Debit', 'Credit', 'Balance'];
      csv = {
        name: `trial-balance-${p}.csv`,
        text: toCsv([
          header,
          ...rows.map((r) => [r.account_code, r.balance.currency, toPlainMajor(r.debit.minor, r.debit.currency), toPlainMajor(r.credit.minor, r.credit.currency), toPlainMajor(r.balance.minor, r.balance.currency)]),
        ]),
      };
      body = (
        <div class="table-wrap">
          <table class="ledger cards">
            <caption class="sr-only">Trial balance for {p}</caption>
            <thead>
              <tr>
                <th scope="col">Account</th>
                <th scope="col" class="num">Debit</th>
                <th scope="col" class="num">Credit</th>
                <th scope="col" class="num">Balance</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <tr key={`${r.account_code}-${r.balance.currency}`}>
                  <td class="main code">{r.account_code}</td>
                  <td class="num" data-label="Debit"><Money minor={r.debit.minor} currency={r.debit.currency} /></td>
                  <td class="num" data-label="Credit"><Money minor={r.credit.minor} currency={r.credit.currency} /></td>
                  <td class="num" data-label="Balance"><Money minor={r.balance.minor} currency={r.balance.currency} /></td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      );
    }
  } else if (state.data?.kind === 'pl') {
    const pl = state.data.pl;
    const lines = [
      ['Revenue', pl.revenue],
      ['Expenses', pl.expenses],
      ['Net income', pl.net_income],
    ] as const;
    csv = {
      name: `profit-and-loss-${pl.period}.csv`,
      text: toCsv([['Line', 'Currency', 'Amount'], ...lines.map(([label, a]) => [label, a.currency, toPlainMajor(a.minor, a.currency)])]),
    };
    body = (
      <div class="table-wrap">
        <table class="ledger cards">
          <caption class="sr-only">Profit and loss for {pl.period}</caption>
          <thead>
            <tr>
              <th scope="col">Line</th>
              <th scope="col" class="num">Amount</th>
            </tr>
          </thead>
          <tbody>
            {lines.map(([label, a]) => (
              <tr key={label}>
                <td class="main">{label}</td>
                <td class="num" data-label="Amount"><Money minor={a.minor} currency={a.currency} /></td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    );
  } else if (state.data?.kind === 'bs') {
    const bs = state.data.bs;
    const lines = [
      ['Assets', bs.assets],
      ['Liabilities', bs.liabilities],
      ['Equity accounts', bs.equity_accounts],
      ['Current year result', bs.current_year_result],
      ['Equity', bs.equity],
    ] as const;
    csv = {
      name: `balance-sheet-${bs.as_of}.csv`,
      text: toCsv([['Line', 'Currency', 'Amount'], ...lines.map(([label, a]) => [label, a.currency, toPlainMajor(a.minor, a.currency)])]),
    };
    body = (
      <div class="table-wrap">
        <table class="ledger cards">
          <caption class="sr-only">Balance sheet as of {bs.as_of}</caption>
          <thead>
            <tr>
              <th scope="col">Line</th>
              <th scope="col" class="num">Amount</th>
            </tr>
          </thead>
          <tbody>
            {lines.map(([label, a]) => (
              <tr key={label}>
                <td class="main">{label}</td>
                <td class="num" data-label="Amount"><Money minor={a.minor} currency={a.currency} /></td>
              </tr>
            ))}
          </tbody>
          <tfoot>
            <tr>
              <td class="main">Assets minus liabilities and equity</td>
              <td class="num" data-label="Check">{formatMinor(BigInt(bs.difference.minor), bs.difference.currency, LOCALE)}</td>
            </tr>
          </tfoot>
        </table>
      </div>
    );
  }

  return (
    <>
      <PageHead title="Reports" actions={csv ? <CsvButton name={csv.name} text={csv.text} /> : undefined}>
        Statements for {period}. Balance sheet is as of the last day of the period. Exports are plain CSV in major units.
      </PageHead>
      <section class="panel" aria-label="Reports">
        <div class="panel-head">{tabs}</div>
        <div id="report-panel" role="tabpanel" aria-labelledby={`tab-${kind}`}>
          {body}
        </div>
      </section>
    </>
  );
}

function CsvButton({ name, text }: { name: string; text: string }) {
  const download = () => {
    const url = URL.createObjectURL(new Blob([text], { type: 'text/csv;charset=utf-8' }));
    const link = document.createElement('a');
    link.href = url;
    link.download = name;
    document.body.appendChild(link);
    link.click();
    link.remove();
    window.setTimeout(() => URL.revokeObjectURL(url), 1000);
  };
  return (
    <button type="button" class="btn" onClick={download}>
      Export CSV
    </button>
  );
}
