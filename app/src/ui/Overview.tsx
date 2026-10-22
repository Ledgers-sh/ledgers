import { useState } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import { reportsBalanceSheet, reportsTrends, reportsTrialBalance, reportsUnitsSummary } from '../api/commands';
import { unitQuery } from '../lib/units';
import { UnitsComparison } from './UnitsComparison';
import type { TrendMonth, TrendsOutput } from '../api/types';
import { useSession } from '../session';
import { useAsync } from '../hooks';
import { periodEnd } from '../lib/period';
import { formatMinor } from '../lib/money';
import {
  averageMonthlyExpenses,
  changeOf,
  endBalance,
  formatRunway,
  monthCount,
  previousRange,
  rollingYear,
  runwayMonths,
  sumOf,
  type Change,
} from '../lib/trends';
import { accountName, formatDate, formatPeriod, intlLocale, t, tRich, type MessageKey } from '../i18n';
import { Ltr } from '../i18n/Ltr';
import { ActivityChart, CashChart, DimensionShares, FlowChart, Sparkline } from './charts';
import { Empty, ErrorBanner, Money, PageHead, Skeleton } from './common';
import { WaitingForYou } from './Approvals';

type Dimension = 'cost_centre' | 'project';

/** Which way a change is good for the business: costs going up is bad, cash going up is good. */
type Good = 'up' | 'down' | null;

interface Kpi {
  id: string;
  label: MessageKey;
  value: bigint;
  previous: bigint;
  spark: bigint[];
  tone: 'rev' | 'exp' | 'net' | 'cash' | 'neutral';
  good: Good;
}

/** The change line under a KPI value, phrased and coloured by what it means. */
function changeText(change: Change): { text: string; tone: 'good' | 'bad' | 'neutral' } {
  if (change.direction === 'flat') return { text: t('kpi.flat'), tone: 'neutral' };
  if (change.direction === 'new') return { text: t('kpi.new'), tone: 'neutral' };
  const arrow = change.direction === 'up' ? '↑' : '↓';
  const percent = new Intl.NumberFormat(intlLocale(), { maximumFractionDigits: 1 }).format(change.percent ?? 0);
  return { text: t('kpi.change', { arrow, percent }), tone: 'neutral' };
}

function toneFor(change: Change, good: Good): 'good' | 'bad' | 'neutral' {
  if (good === null || change.direction === 'flat' || change.direction === 'new') return 'neutral';
  return change.direction === good ? 'good' : 'bad';
}

function KpiCard({ kpi, currency, sub }: { kpi: Kpi; currency: string; sub: string }) {
  const change = changeOf(kpi.value, kpi.previous);
  const tone = toneFor(change, kpi.good);
  const { text } = changeText(change);
  return (
    <div class={`kpi ${kpi.tone}`}>
      <span class="kpi-label">{t(kpi.label)}</span>
      <Money class="kpi-value" minor={kpi.value} currency={currency} />
      <span class={`kpi-change ${tone}`}>{text}</span>
      <span class="kpi-sub">{sub}</span>
      <Sparkline values={kpi.spark} tone={kpi.tone} />
    </div>
  );
}

/** A small "i" that explains a figure on hover or focus, linked to the figure by aria-describedby. */
function InfoTip({ id, label, children }: { id: string; label: string; children: ComponentChildren }) {
  return (
    <span class="infotip">
      <button type="button" class="infotip-button" aria-label={label} aria-describedby={id}>
        <span aria-hidden="true">i</span>
      </button>
      <span id={id} role="tooltip" class="infotip-text">
        {children}
      </span>
    </span>
  );
}

export function Overview() {
  const { client, range, approvalCount, unit, units } = useSession();
  const [dimension, setDimension] = useState<Dimension>('cost_centre');
  const previous = previousRange(range);
  const year = rollingYear(range.to);
  const scope = unitQuery(unit);
  const state = useAsync(
    async () => {
      const [current, before, twelve, trial, bs] = await Promise.all([
        reportsTrends(client, { from: range.from, to: range.to, dimension, unit: scope }),
        reportsTrends(client, { ...previous, unit: scope }),
        reportsTrends(client, { ...year, unit: scope }),
        reportsTrialBalance(client, range.to, scope),
        reportsBalanceSheet(client, periodEnd(range.to), scope),
      ]);
      return { current, before, twelve, trial, bs };
    },
    [client.baseUrl, client.apiKey, range.from, range.to, dimension, scope],
  );
  // The side-by-side units are shown for the company only; the month is the period picker's last month.
  const comparison = useAsync(
    () => (units.length > 0 && unit === null ? reportsUnitsSummary(client, range.to) : Promise.resolve(null)),
    [client.baseUrl, client.apiKey, range.to, units.length, unit],
  );

  if (state.loading && !state.data) return <Skeleton rows={6} label={t('overview.loading')} />;
  if (state.error && !state.data) return <ErrorBanner error={state.error} onRetry={state.reload} />;
  if (!state.data) return null;
  const { current, before, twelve, trial, bs } = state.data;
  return (
    <Dashboard
      current={current}
      before={before}
      twelve={twelve}
      trialRows={trial.rows.map((row) => ({ debit: row.debit, credit: row.credit }))}
      balanced={bs.balanced}
      difference={bs.difference}
      asOf={bs.as_of}
      approvalCount={approvalCount}
      dimension={dimension}
      onDimension={setDimension}
      rangeLabel={range.from === range.to ? formatPeriod(range.from) : `${formatPeriod(range.from)} – ${formatPeriod(range.to)}`}
      months={monthCount(range)}
      comparison={
        units.length > 0 && unit === null ? (
          <UnitsComparison
            units={units}
            summary={comparison.data ?? null}
            error={comparison.error}
            loading={comparison.loading}
            onRetry={comparison.reload}
          />
        ) : null
      }
    />
  );
}

interface DashboardProps {
  current: TrendsOutput;
  before: TrendsOutput;
  twelve: TrendsOutput;
  trialRows: { debit: { minor: string; currency: string }; credit: { minor: string; currency: string } }[];
  balanced: boolean;
  difference: { minor: string; currency: string };
  asOf: string;
  approvalCount: number | null;
  dimension: Dimension;
  onDimension: (dimension: Dimension) => void;
  rangeLabel: string;
  months: number;
  /** The side-by-side business units, when the company is consolidated and has units. */
  comparison: ComponentChildren;
}

function Dashboard(props: DashboardProps) {
  const { current, before, twelve, trialRows, balanced, difference, asOf, approvalCount, dimension, onDimension, rangeLabel, months, comparison } = props;
  const currency = current.currency;
  const locale = intlLocale();
  const windowMonths: TrendMonth[] = current.months;
  const beforeMonths: TrendMonth[] = before.months;
  const yearMonths: TrendMonth[] = twelve.months;

  const hasActivity =
    current.activity.length > 0 || windowMonths.some((month) => month.revenue.minor !== '0' || month.expenses.minor !== '0');
  const sub = rangeLabel;

  const revenue = sumOf(windowMonths, 'revenue');
  const expenses = sumOf(windowMonths, 'expenses');
  const net = sumOf(windowMonths, 'net_result');
  const cash = endBalance(windowMonths, 'cash');
  const receivables = endBalance(windowMonths, 'receivables');
  const payables = endBalance(windowMonths, 'payables');
  const average = averageMonthlyExpenses(windowMonths);
  const runway = runwayMonths(cash, average);

  const kpis: Kpi[] = [
    { id: 'revenue', label: 'term.revenue', value: revenue, previous: sumOf(beforeMonths, 'revenue'), spark: yearMonths.map((m) => BigInt(m.revenue.minor)), tone: 'rev', good: 'up' },
    { id: 'expenses', label: 'term.expenses', value: expenses, previous: sumOf(beforeMonths, 'expenses'), spark: yearMonths.map((m) => BigInt(m.expenses.minor)), tone: 'exp', good: 'down' },
    { id: 'net', label: 'term.net_income', value: net, previous: sumOf(beforeMonths, 'net_result'), spark: yearMonths.map((m) => BigInt(m.net_result.minor)), tone: 'net', good: 'up' },
    { id: 'cash', label: 'term.cash', value: cash, previous: endBalance(beforeMonths, 'cash'), spark: yearMonths.map((m) => BigInt(m.cash.minor)), tone: 'cash', good: 'up' },
    { id: 'receivables', label: 'term.receivables', value: receivables, previous: endBalance(beforeMonths, 'receivables'), spark: yearMonths.map((m) => BigInt(m.receivables.minor)), tone: 'neutral', good: 'down' },
    { id: 'payables', label: 'term.payables', value: payables, previous: endBalance(beforeMonths, 'payables'), spark: yearMonths.map((m) => BigInt(m.payables.minor)), tone: 'neutral', good: null },
  ];

  const flowSummary = t('chart.flow_summary', {
    months: String(months),
    revenue: formatMinor(revenue, currency, locale),
    expenses: formatMinor(expenses, currency, locale),
    net: formatMinor(net, currency, locale),
  });
  const cashSummary = t('chart.cash_summary', { cash: formatMinor(cash, currency, locale), months: String(months) });
  const activityTotals = current.activity.reduce(
    (acc, row) => ({ agent: acc.agent + row.agent, human: acc.human + row.human }),
    { agent: 0, human: 0 },
  );
  const activitySummary = t('chart.activity_summary', { agent: String(activityTotals.agent), human: String(activityTotals.human) });

  const activityRows = windowMonths.map((month) => {
    const row = current.activity.find((entry) => entry.period === month.period);
    return { period: month.period, agent: row?.agent ?? 0, human: row?.human ?? 0 };
  });

  const topMax = current.top_expense_accounts[0] ? BigInt(current.top_expense_accounts[0].amount.minor) : 0n;
  const share = (bps: number) => new Intl.NumberFormat(locale, { style: 'percent', maximumFractionDigits: 1 }).format(bps / 10_000);

  const dimensionRows = current.by_dimension.map((row) => ({ code: row.code, name: row.name, amount: BigInt(row.expenses.minor) }));
  const dimensionTotal = dimensionRows.reduce((sum, row) => sum + row.amount, 0n);
  const dimensionSummary = t('chart.dimension_summary', {
    dimension: t(dimension === 'cost_centre' ? 'chart.cost_centre' : 'chart.project'),
    total: formatMinor(dimensionTotal, currency, locale),
  });

  const trialTotals = new Map<string, { debit: bigint; credit: bigint }>();
  for (const row of trialRows) {
    const entry = trialTotals.get(row.debit.currency) ?? { debit: 0n, credit: 0n };
    entry.debit += BigInt(row.debit.minor);
    entry.credit += BigInt(row.credit.minor);
    trialTotals.set(row.debit.currency, entry);
  }

  return (
    <>
      <PageHead title={t('overview.title')}>{t('overview.intro')}</PageHead>

      {approvalCount !== null && approvalCount > 0 && <WaitingForYou count={approvalCount} />}

      {!hasActivity && (
        <Empty
          title={t('overview.empty_title', { period: rangeLabel })}
          action={
            <a class="btn btn-primary" href="#/new">
              {t('action.post_first')}
            </a>
          }
        >
          {t('overview.empty_body')}
        </Empty>
      )}

      <section class="kpis" aria-labelledby="kpi-heading">
        <h2 id="kpi-heading" class="sr-only">
          {t('overview.kpi_heading')}
        </h2>
        {kpis.map((kpi) => (
          <KpiCard key={kpi.id} kpi={kpi} currency={currency} sub={sub} />
        ))}
      </section>

      {comparison}

      <div class="grid-main">
        <section class="panel chart-panel">
          <FlowChart
            id="flow"
            title={t('overview.flow_title')}
            summary={flowSummary}
            months={windowMonths}
            currency={currency}
          />
        </section>

        <section class="panel chart-panel">
          <div class="runway">
            <div class="runway-head">
              <span class="kpi-label">{t('overview.runway_label')}</span>
              {runway === null ? (
                <span class="runway-value muted">{t('overview.runway_none')}</span>
              ) : (
                <span class="runway-value">
                  {tRich('overview.runway_value', { months: <Ltr>{formatRunway(runway)}</Ltr> })}
                </span>
              )}
              <InfoTip id="runway-tip" label={t('overview.runway_how')}>
                {t('overview.runway_explain')}
              </InfoTip>
            </div>
          </div>
          <CashChart id="cash" title={t('overview.cash_title')} summary={cashSummary} months={windowMonths} currency={currency} />
        </section>
      </div>

      <div class="grid-three">
        <section class="panel" aria-labelledby="where-title">
          <div class="panel-head">
            <h2 id="where-title" class="grow">
              {t('overview.where_title')}
            </h2>
          </div>
          <div class="panel-body">
            <ol class="rank">
              {current.top_expense_accounts.length === 0 && <li class="muted">{t('overview.no_expenses')}</li>}
              {current.top_expense_accounts.map((row) => {
                const amount = BigInt(row.amount.minor);
                const width = topMax > 0n ? Number((amount * 10_000n) / topMax) / 100 : 0;
                return (
                  <li key={row.account}>
                    <a class="rank-row" href={`#/accounts/${encodeURIComponent(row.account)}`}>
                      <span class="rank-name">
                        <span>{accountName(row.name_key)}</span>
                        <span class="mono muted">{row.account}</span>
                      </span>
                      <span class="rank-bar" aria-hidden="true">
                        <span style={{ width: `${width}%` }} />
                      </span>
                      <Money class="rank-amount" minor={amount} currency={currency} />
                      <span class="rank-share">{share(row.share_bps)}</span>
                    </a>
                  </li>
                );
              })}
            </ol>

            <div class="dimension-switch" role="group" aria-label={t('overview.dimension_label')}>
              <button type="button" aria-pressed={dimension === 'cost_centre' ? 'true' : 'false'} onClick={() => onDimension('cost_centre')}>
                {t('chart.cost_centre')}
              </button>
              <button type="button" aria-pressed={dimension === 'project' ? 'true' : 'false'} onClick={() => onDimension('project')}>
                {t('chart.project')}
              </button>
            </div>
            <DimensionShares
              id="dims"
              title={t('overview.dimension_title')}
              summary={dimensionSummary}
              rows={dimensionRows}
              currency={currency}
            />
          </div>
        </section>

        <section class="panel chart-panel">
          <ActivityChart id="activity" title={t('overview.activity_title')} summary={activitySummary} rows={activityRows} />
        </section>

        <section class="panel" aria-labelledby="check-title">
          <div class="panel-head">
            <h2 id="check-title" class="grow">
              {t('overview.check_heading')}
            </h2>
          </div>
          <div class="panel-body">
            <p class="muted" style={{ marginBottom: '12px' }}>
              {t('overview.check_body')}
            </p>
            {trialTotals.size === 0 ? (
              <p class="muted">{t('overview.no_activity')}</p>
            ) : (
              [...trialTotals.entries()].map(([code, sums]) => {
                const ok = sums.debit === sums.credit;
                return (
                  <p key={code} class="check-line">
                    <span class="muted">{t('overview.debits_currency', { currency: code })}</span>
                    <Ltr>{formatMinor(sums.debit, code, locale)}</Ltr>
                    <span class="muted">{t('overview.credits_currency', { currency: code })}</span>
                    <Ltr>{formatMinor(sums.credit, code, locale)}</Ltr>
                    <span class={`badge ${ok ? 'balanced' : 'reversed'}`}>{ok ? t('overview.balanced') : t('overview.not_balanced')}</span>
                  </p>
                );
              })
            )}
            <p class="check-line">
              <span class="muted">{t('overview.as_of', { date: formatDate(asOf) })}</span>
              <span class={`badge ${balanced ? 'balanced' : 'reversed'}`}>
                {balanced ? (
                  t('overview.check_ok')
                ) : (
                  tRich('overview.check_gap', { amount: <Ltr>{formatMinor(BigInt(difference.minor), difference.currency, locale)}</Ltr> })
                )}
              </span>
            </p>
          </div>
        </section>
      </div>
    </>
  );
}
