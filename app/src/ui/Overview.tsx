import { reportsBalanceSheet, reportsProfitAndLoss, reportsTrialBalance } from '../api/commands';
import { useSession } from '../session';
import { useAsync } from '../hooks';
import { periodEnd } from '../lib/period';
import { formatMinor } from '../lib/money';
import { formatDate, formatPeriod, intlLocale, t, tRich } from '../i18n';
import { Ltr } from '../i18n/Ltr';
import { Empty, ErrorBanner, Money, PageHead, Skeleton } from './common';

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

  if (state.loading && !state.data) return <Skeleton rows={6} label={t('overview.loading')} />;
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
  const asOf = t('overview.as_of', { date: formatDate(bs.as_of) });
  const locale = intlLocale();

  return (
    <>
      <PageHead title={t('overview.title')}>{t('overview.intro')}</PageHead>

      {!hasActivity ? (
        <Empty
          title={t('overview.empty_title', { period: formatPeriod(period) })}
          action={
            <a class="btn btn-primary" href="#/new">
              {t('action.post_first')}
            </a>
          }
        >
          {t('overview.empty_body')}
        </Empty>
      ) : null}

      <section aria-labelledby="pl-heading">
        <h2 id="pl-heading" class="sr-only">
          {t('overview.pl_heading')}
        </h2>
        <div class="tiles">
          <div class="tile">
            <span class="label">{t('term.revenue')}</span>
            <Money class="value" minor={pl.revenue.minor} currency={pl.revenue.currency} />
            <span class="sub">{formatPeriod(pl.period)}</span>
          </div>
          <div class="tile">
            <span class="label">{t('term.expenses')}</span>
            <Money class="value" minor={pl.expenses.minor} currency={pl.expenses.currency} />
            <span class="sub">{formatPeriod(pl.period)}</span>
          </div>
          <div class="tile agent">
            <span class="label">{t('term.net_income')}</span>
            <Money class="value" minor={pl.net_income.minor} currency={pl.net_income.currency} />
            <span class="sub">{t('overview.net_income_sub')}</span>
          </div>
        </div>
      </section>

      <section aria-labelledby="bs-heading">
        <h2 id="bs-heading" class="sr-only">
          {t('overview.bs_heading')}
        </h2>
        <div class="tiles">
          <div class="tile">
            <span class="label">{t('term.assets')}</span>
            <Money class="value" minor={bs.assets.minor} currency={bs.assets.currency} />
            <span class="sub">{asOf}</span>
          </div>
          <div class="tile">
            <span class="label">{t('term.liabilities')}</span>
            <Money class="value" minor={bs.liabilities.minor} currency={bs.liabilities.currency} />
            <span class="sub">{asOf}</span>
          </div>
          <div class="tile">
            <span class="label">{t('term.equity')}</span>
            <Money class="value" minor={bs.equity.minor} currency={bs.equity.currency} />
            <span class="sub">{asOf}</span>
          </div>
        </div>
      </section>

      <div class="two-col">
        <section class="panel" aria-labelledby="tb-heading">
          <div class="panel-head">
            <h2 id="tb-heading" class="grow">
              {t('overview.tb_heading')}
            </h2>
            <a class="btn btn-sm btn-ghost" href="#/reports">
              {t('overview.open_reports')}
            </a>
          </div>
          <div class="panel-body">
            {hasActivity ? (
              <dl class="entry-meta" style={{ gridTemplateColumns: 'repeat(auto-fit, minmax(150px, 1fr))' }}>
                {[...totals.entries()].map(([currency, sums]) => {
                  const balanced = sums.debit === sums.credit;
                  return (
                    <div key={currency}>
                      <dt>{t('overview.debits_currency', { currency })}</dt>
                      <dd>
                        <Ltr>{formatMinor(sums.debit, currency, locale)}</Ltr>
                      </dd>
                      <dt style={{ marginTop: '10px' }}>{t('overview.credits_currency', { currency })}</dt>
                      <dd>
                        <Ltr>{formatMinor(sums.credit, currency, locale)}</Ltr>
                      </dd>
                      <dd style={{ marginTop: '10px', fontFamily: 'var(--sans)' }}>
                        <span class={`badge ${balanced ? 'balanced' : 'reversed'}`}>
                          {balanced
                            ? t('overview.balanced')
                            : tRich('overview.out_by', { amount: <Ltr>{formatMinor(sums.debit - sums.credit, currency, locale)}</Ltr> })}
                        </span>
                      </dd>
                    </div>
                  );
                })}
              </dl>
            ) : (
              <p class="muted">{t('overview.no_activity')}</p>
            )}
          </div>
        </section>

        <section class="panel" aria-labelledby="eq-heading">
          <div class="panel-head">
            <h2 id="eq-heading" class="grow">
              {t('overview.check_heading')}
            </h2>
          </div>
          <div class="panel-body">
            <p class="muted" style={{ marginBottom: '12px' }}>
              {t('overview.check_body')}
            </p>
            <span class={`badge ${balanceSheetGap === 0n ? 'balanced' : 'reversed'}`}>
              {balanceSheetGap === 0n ? (
                t('overview.check_ok')
              ) : (
                tRich('overview.check_gap', { amount: <Ltr>{formatMinor(balanceSheetGap, bs.assets.currency, locale)}</Ltr> })
              )}
            </span>
          </div>
        </section>
      </div>
    </>
  );
}
