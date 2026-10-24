import { useState } from 'preact/hooks';
import { reportsCostsByUnit } from '../api/connect';
import { useSession } from '../session';
import { formatPeriod, t } from '../i18n';
import { useAsync } from '../hooks';
import { costView, monthsBetween, type CostView } from '../lib/connect';
import { formatMinor } from '../lib/money';
import { ErrorBanner, Skeleton } from './common';
import { Figure, useWidth } from './charts';
import { colourOf } from '../lib/units';

const SOURCE_NAMES: Record<string, string> = {
  cloudflare: 'Cloudflare',
  anthropic: 'Anthropic',
  planetscale: 'PlanetScale',
  aws: 'Amazon Web Services',
  vercel: 'Vercel',
  gcp: 'Google Cloud',
  azure: 'Microsoft Azure',
  supabase: 'Supabase',
  openai: 'OpenAI',
  openrouter: 'OpenRouter',
  stripe: 'Stripe',
};

/** Stacked bars per month, one segment per series (a unit or a source). Same frame as the other charts. */
export function CostStack({ view, id, title, summary, currency }: { view: CostView; id: string; title: string; summary: string; currency: string }) {
  const { ref, width } = useWidth(640);
  const height = 220;
  const padLeft = 56;
  const padRight = 8;
  const padTop = 12;
  const padBottom = 28;
  const plotWidth = Math.max(120, width - padLeft - padRight);
  const plotHeight = height - padTop - padBottom;
  const count = view.rows.length;
  const band = plotWidth / Math.max(count, 1);
  const barWidth = Math.min(28, band * 0.62);
  const top = Math.max(1, ...view.rows.map((row) => row.total));
  const y = (value: number) => padTop + plotHeight - (value / top) * plotHeight;
  const tickValues = [0, top / 2, top];

  const legend = (
    <div class="chart-legend" aria-hidden="true">
      {view.series.map((series) => (
        <span key={series.key} class="legend-item">
          <span class="swatch" style={{ background: series.colour }} />
          {series.label}
        </span>
      ))}
    </div>
  );

  const table = (
    <table>
      <caption>{title}</caption>
      <thead>
        <tr>
          <th scope="col">{t('costs.month')}</th>
          {view.series.map((series) => (
            <th key={series.key} scope="col">
              {series.label}
            </th>
          ))}
          <th scope="col">{t('costs.total')}</th>
        </tr>
      </thead>
      <tbody>
        {view.rows.map((row) => (
          <tr key={row.period}>
            <th scope="row">{formatPeriod(row.period)}</th>
            {view.series.map((series) => (
              <td key={series.key}>{formatMinor(BigInt(row.values[series.key] ?? 0), currency)}</td>
            ))}
            <td>{formatMinor(BigInt(row.total), currency)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );

  return (
    <Figure id={id} title={title} summary={summary} legend={legend} table={table}>
      <div class="chart-body" ref={ref} dir="ltr">
        <svg class="chart-svg" viewBox={`0 0 ${width} ${height}`} width={width} height={height} role="img" aria-hidden="true" focusable="false">
          {tickValues.map((value) => (
            <g key={value}>
              <line class="ch-grid" x1={padLeft} x2={width - padRight} y1={y(value)} y2={y(value)} />
              <text class="ch-axis" x={padLeft - 8} y={y(value)} text-anchor="end" dominant-baseline="middle">
                {formatMinor(BigInt(Math.round(value)), currency)}
              </text>
            </g>
          ))}
          {view.rows.map((row, index) => {
            const x = padLeft + band * index + (band - barWidth) / 2;
            let base = 0;
            return (
              <g key={row.period}>
                {view.series.map((series) => {
                  const value = row.values[series.key] ?? 0;
                  if (value <= 0) return null;
                  const top = y(base + value);
                  const bottom = y(base);
                  base += value;
                  return <rect key={series.key} x={x} y={top} width={barWidth} height={Math.max(1, bottom - top)} fill={series.colour} />;
                })}
                <text class="ch-axis" x={x + barWidth / 2} y={height - 8} text-anchor="middle">
                  {formatPeriod(row.period).slice(0, 3)}
                </text>
              </g>
            );
          })}
        </svg>
      </div>
    </Figure>
  );
}

/** Overview's "Costs by venture": the connected costs of the range, stacked by unit or by source. */
export function CostsByVenture({ from, to, currency }: { from: string; to: string; currency: string }) {
  const { client, units } = useSession();
  const [mode, setMode] = useState<'unit' | 'source'>('unit');
  const state = useAsync(() => reportsCostsByUnit(client, from, to), [client.baseUrl, client.apiKey, from, to]);
  if (state.loading && !state.data) return <Skeleton rows={3} label={t('costs.loading')} />;
  if (state.error && !state.data) return <ErrorBanner error={state.error} onRetry={state.reload} />;
  if (!state.data) return null;
  const lines = state.data.lines;
  if (lines.length === 0) {
    return (
      <section class="panel chart-panel costs-empty" aria-labelledby="costs-empty-title">
        <h3 id="costs-empty-title" class="chart-title">{t('costs.title')}</h3>
        <p class="muted">{t('costs.empty')}</p>
      </section>
    );
  }
  const unitLabels = Object.fromEntries(units.map((unit) => [unit.code, { name: unit.name, colour: colourOf(unit) }]));
  const view = costView({
    lines,
    periods: monthsBetween(from, to),
    mode,
    baseCurrency: currency,
    unitLabels,
    sourceLabels: SOURCE_NAMES,
  });
  const summary = t('costs.summary', {
    from: formatPeriod(from),
    to: formatPeriod(to),
    total: formatMinor(BigInt(view.total), currency),
    units: String(new Set(lines.map((line) => line.unit_code)).size),
    sources: String(new Set(lines.map((line) => line.connector)).size),
  });
  const others = Object.entries(view.otherCurrencies);
  return (
    <div class="costs">
      <div class="costs-head">
        <div class="segmented" role="group" aria-label={t('costs.stack_by')}>
          <button type="button" class="btn btn-sm" aria-pressed={mode === 'unit'} onClick={() => setMode('unit')}>
            {t('costs.by_unit')}
          </button>
          <button type="button" class="btn btn-sm" aria-pressed={mode === 'source'} onClick={() => setMode('source')}>
            {t('costs.by_source')}
          </button>
        </div>
      </div>
      <section class="panel chart-panel">
        <CostStack id="costs" title={t('costs.title')} summary={summary} view={view} currency={currency} />
      </section>
      {others.map(([code, amount]) => (
        <p key={code} class="note">
          {t('costs.other_currency', { currency: code, amount: formatMinor(BigInt(amount), code) })}
        </p>
      ))}
    </div>
  );
}
