import { reportsBalanceSheet, reportsProfitAndLoss, reportsTrialBalance } from '../api/commands';
import { useSession } from '../session';
import { useAsync } from '../hooks';
import { periodEnd } from '../lib/period';
import { formatMinor } from '../lib/money';
import { Empty, ErrorBanner, LOCALE, Money, PageHead, Skeleton } from './common';

export function Overview() {
  const { client, period } = useSession();
  const state = useAsync(
    async () => {
      const [trial, pl, bs] = await Promise.all([
        reportsTrialBalance(client, period),
        reportsProfitAndLoss(client, period),
        reportsBalanceSheet(client, periodEnd(period)),
      ]);
      return { trial, pl, bs };
    },
    [client.baseUrl, client.apiKey, period],
  );

  if (state.loading && !state.data) return <Skeleton rows={6} label="Loading overview" />;
  if (state.error && !state.data) return <ErrorBanner error={state.error} onRetry={state.reload} />;
  if (!state.data) return null;
  const { trial, pl, bs } = state.data;

  // Totals per currency: a balanced trial balance has debits equal to credits in each currency.
  const totals = new Map<string, { debit: bigint; credit: bigint }>();
  for (const row of trial.rows) {
    const entry = totals.get(row.debit.currency) ?? { debit: 0n, credit: 0n };
    entry.debit += BigInt(row.debit.minor);
    entry.credit += BigInt(row.credit.minor);
    totals.set(row.debit.currency, entry);
  }
  const hasActivity = trial.rows.length > 0;
  const balanceSheetGap = BigInt(bs.assets.minor) - (BigInt(bs.liabilities.minor) + BigInt(bs.equity.minor));

  return (
    <>
      <PageHead title="Overview">
        Trial balance, profit and loss and balance sheet for the selected period.
      </PageHead>

      {!hasActivity ? (
        <Empty
          title={`No entries in ${period} yet`}
          action={
            <a class="btn btn-primary" href="#/new">
              Post your first entry
            </a>
          }
        >
          Once an entry is posted, its effect on revenue, balances and the trial balance appears here. Pick another period to look back.
        </Empty>
      ) : null}

      <section aria-labelledby="pl-heading">
        <h2 id="pl-heading" class="sr-only">
          Profit and loss
        </h2>
        <div class="tiles">
          <div class="tile">
            <span class="label">Revenue</span>
            <Money class="value" minor={pl.revenue.minor} currency={pl.revenue.currency} />
            <span class="sub">{pl.period}</span>
          </div>
          <div class="tile">
            <span class="label">Expenses</span>
            <Money class="value" minor={pl.expenses.minor} currency={pl.expenses.currency} />
            <span class="sub">{pl.period}</span>
          </div>
          <div class="tile agent">
            <span class="label">Net income</span>
            <Money class="value" minor={pl.net_income.minor} currency={pl.net_income.currency} />
            <span class="sub">Revenue minus expenses</span>
          </div>
        </div>
      </section>

      <section aria-labelledby="bs-heading">
        <h2 id="bs-heading" class="sr-only">
          Balance sheet
        </h2>
        <div class="tiles">
          <div class="tile">
            <span class="label">Assets</span>
            <Money class="value" minor={bs.assets.minor} currency={bs.assets.currency} />
            <span class="sub">As of {bs.as_of}</span>
          </div>
          <div class="tile">
            <span class="label">Liabilities</span>
            <Money class="value" minor={bs.liabilities.minor} currency={bs.liabilities.currency} />
            <span class="sub">As of {bs.as_of}</span>
          </div>
          <div class="tile">
            <span class="label">Equity</span>
            <Money class="value" minor={bs.equity.minor} currency={bs.equity.currency} />
            <span class="sub">As of {bs.as_of}</span>
          </div>
        </div>
      </section>

      <div class="two-col">
        <section class="panel" aria-labelledby="tb-heading">
          <div class="panel-head">
            <h2 id="tb-heading" class="grow">Trial balance</h2>
            <a class="btn btn-sm btn-ghost" href="#/reports">
              Open reports
            </a>
          </div>
          <div class="panel-body">
            {hasActivity ? (
              <dl class="entry-meta" style={{ gridTemplateColumns: 'repeat(auto-fit, minmax(150px, 1fr))' }}>
                {[...totals.entries()].map(([currency, sums]) => {
                  const balanced = sums.debit === sums.credit;
                  return (
                    <div key={currency}>
                      <dt>Debits, {currency}</dt>
                      <dd>{formatMinor(sums.debit, currency, LOCALE)}</dd>
                      <dt style={{ marginTop: '10px' }}>Credits, {currency}</dt>
                      <dd>{formatMinor(sums.credit, currency, LOCALE)}</dd>
                      <dd style={{ marginTop: '10px', fontFamily: 'var(--sans)' }}>
                        <span class={`badge ${balanced ? 'balanced' : 'reversed'}`}>
                          {balanced ? 'Balanced' : `Out by ${formatMinor(sums.debit - sums.credit, currency, LOCALE)}`}
                        </span>
                      </dd>
                    </div>
                  );
                })}
              </dl>
            ) : (
              <p class="muted">No activity in this period.</p>
            )}
          </div>
        </section>

        <section class="panel" aria-labelledby="eq-heading">
          <div class="panel-head">
            <h2 id="eq-heading" class="grow">Balance check</h2>
          </div>
          <div class="panel-body">
            <p class="muted" style={{ marginBottom: '12px' }}>
              Assets should equal liabilities plus equity at the period end.
            </p>
            <span class={`badge ${balanceSheetGap === 0n ? 'balanced' : 'reversed'}`}>
              {balanceSheetGap === 0n ? 'Assets = liabilities + equity' : `Gap ${formatMinor(balanceSheetGap, bs.assets.currency, LOCALE)}`}
            </span>
          </div>
        </section>
      </div>
    </>
  );
}
