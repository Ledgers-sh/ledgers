import { useState } from 'preact/hooks';
import { reportsBalanceSheet, reportsProfitAndLoss, reportsTrialBalance } from '../api/commands';
import { useAsync } from '../hooks';
import { useSession } from '../session';
import { periodEnd } from '../lib/period';
import { formatMinor, toPlainMajor } from '../lib/money';
import { toCsv } from '../lib/csv';
import { ErrorBanner, Money, PageHead, Skeleton, Empty } from './common';
import { formatDate, formatPeriod, intlLocale, t, type MessageKey } from '../i18n';
import { Ltr } from '../i18n/Ltr';

type ReportKind = 'trial' | 'pl' | 'bs';
const TABS: { id: ReportKind; label: MessageKey }[] = [
  { id: 'trial', label: 'reports.tab_trial' },
  { id: 'pl', label: 'reports.tab_pl' },
  { id: 'bs', label: 'reports.tab_bs' },
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
    <div role="tablist" aria-label={t('reports.tabs_label')} class="row-actions" style={{ gap: '4px' }}>
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
          {t(tab.label)}
        </button>
      ))}
    </div>
  );

  let body;
  let csv: { name: string; text: string } | null = null;
  const locale = intlLocale();
  if (state.loading && !state.data) body = <Skeleton rows={6} label={t('reports.loading')} />;
  else if (state.error && !state.data) body = <ErrorBanner error={state.error} onRetry={state.reload} />;
  else if (state.data?.kind === 'trial') {
    const { rows, period: p } = state.data.trial;
    if (rows.length === 0) {
      body = (
        <Empty title={t('reports.empty_title', { period: formatPeriod(p) })}>{t('reports.empty_body')}</Empty>
      );
    } else {
      // The CSV keeps its English column names: it is a file other tools read.
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
            <caption class="sr-only">{t('reports.caption_trial', { period: formatPeriod(p) })}</caption>
            <thead>
              <tr>
                <th scope="col">{t('term.account')}</th>
                <th scope="col" class="num">{t('term.debit')}</th>
                <th scope="col" class="num">{t('term.credit')}</th>
                <th scope="col" class="num">{t('term.balance')}</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <tr key={`${r.account_code}-${r.balance.currency}`}>
                  <td class="main code"><Ltr>{r.account_code}</Ltr></td>
                  <td class="num" data-label={t('term.debit')}><Money minor={r.debit.minor} currency={r.debit.currency} /></td>
                  <td class="num" data-label={t('term.credit')}><Money minor={r.credit.minor} currency={r.credit.currency} /></td>
                  <td class="num" data-label={t('term.balance')}><Money minor={r.balance.minor} currency={r.balance.currency} /></td>
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
      ['term.revenue', pl.revenue],
      ['term.expenses', pl.expenses],
      ['term.net_income', pl.net_income],
    ] as const;
    csv = {
      name: `profit-and-loss-${pl.period}.csv`,
      text: toCsv([['Line', 'Currency', 'Amount'], ...lines.map(([label, a]) => [t(label), a.currency, toPlainMajor(a.minor, a.currency)])]),
    };
    body = (
      <div class="table-wrap">
        <table class="ledger cards">
          <caption class="sr-only">{t('reports.caption_pl', { period: formatPeriod(pl.period) })}</caption>
          <thead>
            <tr>
              <th scope="col">{t('term.line')}</th>
              <th scope="col" class="num">{t('term.amount')}</th>
            </tr>
          </thead>
          <tbody>
            {lines.map(([label, a]) => (
              <tr key={label}>
                <td class="main">{t(label)}</td>
                <td class="num" data-label={t('term.amount')}><Money minor={a.minor} currency={a.currency} /></td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    );
  } else if (state.data?.kind === 'bs') {
    const bs = state.data.bs;
    const lines = [
      ['term.assets', bs.assets],
      ['term.liabilities', bs.liabilities],
      ['term.equity_accounts', bs.equity_accounts],
      ['term.current_year_result', bs.current_year_result],
      ['term.equity', bs.equity],
    ] as const;
    csv = {
      name: `balance-sheet-${bs.as_of}.csv`,
      text: toCsv([['Line', 'Currency', 'Amount'], ...lines.map(([label, a]) => [t(label), a.currency, toPlainMajor(a.minor, a.currency)])]),
    };
    body = (
      <div class="table-wrap">
        <table class="ledger cards">
          <caption class="sr-only">{t('reports.caption_bs', { date: formatDate(bs.as_of) })}</caption>
          <thead>
            <tr>
              <th scope="col">{t('term.line')}</th>
              <th scope="col" class="num">{t('term.amount')}</th>
            </tr>
          </thead>
          <tbody>
            {lines.map(([label, a]) => (
              <tr key={label}>
                <td class="main">{t(label)}</td>
                <td class="num" data-label={t('term.amount')}><Money minor={a.minor} currency={a.currency} /></td>
              </tr>
            ))}
          </tbody>
          <tfoot>
            <tr>
              <td class="main">{t('reports.check_row')}</td>
              <td class="num" data-label={t('term.check')}><Ltr>{formatMinor(BigInt(bs.difference.minor), bs.difference.currency, locale)}</Ltr></td>
            </tr>
          </tfoot>
        </table>
      </div>
    );
  }

  return (
    <>
      <PageHead title={t('reports.title')} actions={csv ? <CsvButton name={csv.name} text={csv.text} /> : undefined}>
        {t('reports.intro', { period: formatPeriod(period) })}
      </PageHead>
      <section class="panel" aria-label={t('reports.section')}>
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
      {t('reports.export')}
    </button>
  );
}
