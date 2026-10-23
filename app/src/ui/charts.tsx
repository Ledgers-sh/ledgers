/**
 * The Overview charts: hand-drawn SVG (see docs/adr/0009). Each chart is a
 * figure with a text summary and a visually hidden data table, so a screen
 * reader gets the numbers without the drawing. Pointer and keyboard both move
 * a crosshair across the months; the tooltip repeats the exact amounts.
 */
import { useEffect, useRef, useState } from 'preact/hooks';
import type { TrendMonth } from '../api/types';
import { formatMinor, currencyExponent } from '../lib/money';
import { domainOf, linear, ticks } from '../lib/scale';
import { formatPeriod, intlLocale, t } from '../i18n';
import { Ltr } from '../i18n/Ltr';

const MONTH_LABEL_WIDTH = 44;

/** Short month name in the reader's language, Western digits kept. */
export function monthShort(period: string): string {
  const year = Number(period.slice(0, 4));
  const month = Number(period.slice(5, 7)) - 1;
  return new Intl.DateTimeFormat(intlLocale(), { month: 'short', timeZone: 'UTC' }).format(new Date(Date.UTC(year, month, 1)));
}

/** Measures an element's width and keeps it current. Falls back to `fallback` until measured. */
export function useWidth(fallback: number) {
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(fallback);
  useEffect(() => {
    const element = ref.current;
    if (!element) return;
    const update = () => setWidth(Math.max(280, Math.round(element.getBoundingClientRect().width)));
    update();
    if (typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(update);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  return { ref, width };
}

/** A minor-unit amount as a number of major units, for axis labels only. */
function major(minor: bigint, currency: string): number {
  return Number(minor) / 10 ** currencyExponent(currency);
}

function compact(value: number): string {
  return new Intl.NumberFormat(intlLocale(), { notation: 'compact', maximumFractionDigits: 1 }).format(value);
}

/** The index of the month under a pointer or a touch, given the plot's left edge and band width. */
function indexAt(clientX: number, rect: DOMRect, left: number, band: number, count: number): number {
  const x = clientX - rect.left - left;
  return Math.min(count - 1, Math.max(0, Math.floor(x / band)));
}

/** Moves the crosshair with the arrow keys (the chart is laid out left to right whatever the page direction). */
function keyStep(key: string, index: number, count: number): number | null {
  if (key === 'ArrowRight') return Math.min(count - 1, index + 1);
  if (key === 'ArrowLeft') return Math.max(0, index - 1);
  if (key === 'Home') return 0;
  if (key === 'End') return count - 1;
  return null;
}

interface FigureProps {
  title: string;
  summary: string;
  children: preact.ComponentChildren;
  table: preact.ComponentChildren;
  legend?: preact.ComponentChildren;
  id: string;
}

/** The chart frame: heading, legend, the drawing and its hidden table. */
export function Figure({ title, summary, children, table, legend, id }: FigureProps) {
  return (
    <figure class="chart" aria-labelledby={`${id}-title`} aria-describedby={`${id}-summary`}>
      <div class="chart-head">
        <figcaption id={`${id}-title`} class="chart-title">
          {title}
        </figcaption>
        {legend}
      </div>
      <p id={`${id}-summary`} class="sr-only">
        {summary}
      </p>
      {children}
      <div class="sr-only">{table}</div>
    </figure>
  );
}

export interface FlowSeries {
  revenue: boolean;
  expenses: boolean;
  net: boolean;
}

/** Revenue and expenses as bars per month, net result as a line. Legend toggles hide a series. */
export function FlowChart({ months, currency, id, title, summary }: { months: TrendMonth[]; currency: string; id: string; title: string; summary: string }) {
  const { ref, width } = useWidth(640);
  const [visible, setVisible] = useState<FlowSeries>({ revenue: true, expenses: true, net: true });
  const [index, setIndex] = useState<number | null>(null);
  const locale = intlLocale();
  const count = months.length;
  const height = 260;
  const padLeft = 56;
  const padRight = 8;
  const padTop = 14;
  const padBottom = 28;
  const plotWidth = Math.max(120, width - padLeft - padRight);
  const plotHeight = height - padTop - padBottom;
  const band = plotWidth / Math.max(count, 1);
  const barWidth = Math.min(20, band * 0.32);
  const values = months.flatMap((month) => [
    ...(visible.revenue ? [major(BigInt(month.revenue.minor), currency)] : []),
    ...(visible.expenses ? [major(BigInt(month.expenses.minor), currency)] : []),
    ...(visible.net ? [major(BigInt(month.net_result.minor), currency)] : []),
  ]);
  const domain = domainOf(values);
  const y = linear(domain, padTop + plotHeight, padTop);
  const zero = y(0);
  const labelEvery = band * count > 0 && band < MONTH_LABEL_WIDTH ? 2 : 1;
  const focus = index !== null && count > 0 ? months[index] : null;
  const tooltipLeft = index === null ? 0 : ((padLeft + band * (index + 0.5)) / width) * 100;

  const toggle = (key: keyof FlowSeries) => setVisible((current) => ({ ...current, [key]: !current[key] }));

  const legend = (
    <div class="chart-legend" role="group" aria-label={t('chart.legend')}>
      <button type="button" class="legend-item rev" aria-pressed={visible.revenue ? 'true' : 'false'} onClick={() => toggle('revenue')}>
        <span class="swatch" aria-hidden="true" />
        {t('chart.revenue')}
      </button>
      <button type="button" class="legend-item exp" aria-pressed={visible.expenses ? 'true' : 'false'} onClick={() => toggle('expenses')}>
        <span class="swatch" aria-hidden="true" />
        {t('chart.expenses')}
      </button>
      <button type="button" class="legend-item net" aria-pressed={visible.net ? 'true' : 'false'} onClick={() => toggle('net')}>
        <span class="swatch line" aria-hidden="true" />
        {t('chart.net_result')}
      </button>
    </div>
  );

  const table = (
    <table>
      <caption>{title}</caption>
      <thead>
        <tr>
          <th scope="col">{t('chart.month')}</th>
          <th scope="col">{t('chart.revenue')}</th>
          <th scope="col">{t('chart.expenses')}</th>
          <th scope="col">{t('chart.net_result')}</th>
        </tr>
      </thead>
      <tbody>
        {months.map((month) => (
          <tr key={month.period}>
            <th scope="row">{formatPeriod(month.period)}</th>
            <td>{formatMinor(month.revenue.minor, currency, locale)}</td>
            <td>{formatMinor(month.expenses.minor, currency, locale)}</td>
            <td>{formatMinor(month.net_result.minor, currency, locale)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );

  const points = months.map((month, i) => `${padLeft + band * (i + 0.5)},${y(visible.net ? major(BigInt(month.net_result.minor), currency) : 0)}`);

  return (
    <Figure id={id} title={title} summary={summary} legend={legend} table={table}>
      <div class="chart-body" ref={ref} dir="ltr">
        {count === 0 ? null : (
          <>
            <svg
              class="chart-svg"
              viewBox={`0 0 ${width} ${height}`}
              width={width}
              height={height}
              role="img"
              aria-hidden="true"
              focusable="false"
              onPointerMove={(event) => {
                const rect = (event.currentTarget as SVGElement).getBoundingClientRect();
                setIndex(indexAt(event.clientX, rect, padLeft, band, count));
              }}
              onPointerDown={(event) => {
                const rect = (event.currentTarget as SVGElement).getBoundingClientRect();
                setIndex(indexAt(event.clientX, rect, padLeft, band, count));
              }}
              onPointerLeave={() => setIndex(null)}
            >
              {ticks(domain, 4).map((value) => (
                <g key={value}>
                  <line class="ch-grid" x1={padLeft} x2={width - padRight} y1={y(value)} y2={y(value)} />
                  <text class="ch-axis" x={padLeft - 8} y={y(value)} text-anchor="end" dominant-baseline="middle">
                    {compact(value)}
                  </text>
                </g>
              ))}
              <line class="ch-zero" x1={padLeft} x2={width - padRight} y1={zero} y2={zero} />
              {months.map((month, i) => {
                const cx = padLeft + band * (i + 0.5);
                const rev = major(BigInt(month.revenue.minor), currency);
                const exp = major(BigInt(month.expenses.minor), currency);
                return (
                  <g key={month.period} class={`ch-col${index === i ? ' focus' : ''}`}>
                    {visible.revenue && (
                      <rect class="ch-rev bar" x={cx - barWidth - 1} y={Math.min(y(rev), zero)} width={barWidth} height={Math.max(1, Math.abs(y(rev) - zero))} rx="3" style={{ animationDelay: `${i * 35}ms` }} />
                    )}
                    {visible.expenses && (
                      <rect class="ch-exp bar" x={cx + 1} y={Math.min(y(exp), zero)} width={barWidth} height={Math.max(1, Math.abs(y(exp) - zero))} rx="3" style={{ animationDelay: `${i * 35 + 60}ms` }} />
                    )}
                    {i % labelEvery === 0 && (
                      <text class="ch-axis" x={cx} y={height - 8} text-anchor="middle">
                        {monthShort(month.period)}
                      </text>
                    )}
                  </g>
                );
              })}
              {visible.net && <polyline class="ch-net-line" points={points.join(' ')} fill="none" />}
              {visible.net &&
                months.map((month, i) => (
                  <circle key={month.period} class={`ch-net-dot${index === i ? ' focus' : ''}`} cx={padLeft + band * (i + 0.5)} cy={y(major(BigInt(month.net_result.minor), currency))} r={index === i ? 4.5 : 3} />
                ))}
              {index !== null && <line class="ch-cross" x1={padLeft + band * (index + 0.5)} x2={padLeft + band * (index + 0.5)} y1={padTop} y2={padTop + plotHeight} />}
            </svg>
            <div
              class="chart-hit"
              tabIndex={0}
              role="group"
              aria-label={t('chart.keys_hint')}
              style={{ insetInlineStart: `${padLeft}px`, insetInlineEnd: `${padRight}px` }}
              onKeyDown={(event) => {
                const next = keyStep(event.key, index ?? (event.key === 'End' ? count - 1 : 0), count);
                if (next !== null) {
                  event.preventDefault();
                  setIndex(next);
                } else if (event.key === 'Escape') {
                  setIndex(null);
                }
              }}
              onFocus={() => index === null && setIndex(count - 1)}
              onBlur={() => setIndex(null)}
            />
            {focus && (
              <div class="chart-tip" style={{ left: `${tooltipLeft}%`, transform: tooltipLeft > 60 ? 'translateX(-100%)' : 'translateX(-8px)' }} aria-hidden="true">
                <strong>{formatPeriod(focus.period)}</strong>
                {visible.revenue && (
                  <span class="tip-row rev">
                    <span>{t('chart.revenue')}</span>
                    <Ltr>{formatMinor(focus.revenue.minor, currency, locale)}</Ltr>
                  </span>
                )}
                {visible.expenses && (
                  <span class="tip-row exp">
                    <span>{t('chart.expenses')}</span>
                    <Ltr>{formatMinor(focus.expenses.minor, currency, locale)}</Ltr>
                  </span>
                )}
                <span class="tip-row net">
                  <span>{t('chart.net_result')}</span>
                  <Ltr>{formatMinor(focus.net_result.minor, currency, locale)}</Ltr>
                </span>
              </div>
            )}
          </>
        )}
      </div>
    </Figure>
  );
}

/** One line over the months: the cash balance at each month end, shaded below. */
export function CashChart({ months, currency, id, title, summary }: { months: TrendMonth[]; currency: string; id: string; title: string; summary: string }) {
  const { ref, width } = useWidth(640);
  const [index, setIndex] = useState<number | null>(null);
  const locale = intlLocale();
  const count = months.length;
  const height = 200;
  const padLeft = 56;
  const padRight = 10;
  const padTop = 12;
  const padBottom = 28;
  const plotWidth = Math.max(120, width - padLeft - padRight);
  const plotHeight = height - padTop - padBottom;
  const band = plotWidth / Math.max(count, 1);
  const values = months.map((month) => major(BigInt(month.cash.minor), currency));
  const domain = domainOf(values);
  const y = linear(domain, padTop + plotHeight, padTop);
  const xs = months.map((_, i) => padLeft + band * (i + 0.5));
  const line = months.map((_, i) => `${i === 0 ? 'M' : 'L'}${xs[i]},${y(values[i])}`).join(' ');
  const area = count > 0 ? `${line} L${xs[count - 1]},${y(0)} L${xs[0]},${y(0)} Z` : '';
  const focus = index !== null ? months[index] : null;
  const tooltipLeft = index === null ? 0 : (xs[index] / width) * 100;

  const table = (
    <table>
      <caption>{title}</caption>
      <thead>
        <tr>
          <th scope="col">{t('chart.month')}</th>
          <th scope="col">{t('chart.cash')}</th>
        </tr>
      </thead>
      <tbody>
        {months.map((month) => (
          <tr key={month.period}>
            <th scope="row">{formatPeriod(month.period)}</th>
            <td>{formatMinor(month.cash.minor, currency, locale)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );

  return (
    <Figure id={id} title={title} summary={summary} table={table}>
      <div class="chart-body" ref={ref} dir="ltr">
        {count > 0 && (
          <>
            <svg
              class="chart-svg"
              viewBox={`0 0 ${width} ${height}`}
              width={width}
              height={height}
              role="img"
              aria-hidden="true"
              focusable="false"
              onPointerMove={(event) => setIndex(indexAt(event.clientX, (event.currentTarget as SVGElement).getBoundingClientRect(), padLeft, band, count))}
              onPointerDown={(event) => setIndex(indexAt(event.clientX, (event.currentTarget as SVGElement).getBoundingClientRect(), padLeft, band, count))}
              onPointerLeave={() => setIndex(null)}
            >
              {ticks(domain, 3).map((value) => (
                <g key={value}>
                  <line class="ch-grid" x1={padLeft} x2={width - padRight} y1={y(value)} y2={y(value)} />
                  <text class="ch-axis" x={padLeft - 8} y={y(value)} text-anchor="end" dominant-baseline="middle">
                    {compact(value)}
                  </text>
                </g>
              ))}
              <path class="ch-cash-area" d={area} />
              <path class="ch-cash-line" d={line} fill="none" />
              {months.map((month, i) => (
                <g key={month.period}>
                  <circle class={`ch-cash-dot${index === i ? ' focus' : ''}`} cx={xs[i]} cy={y(values[i])} r={index === i ? 4.5 : 2.5} />
                  {(count < 8 || i % 2 === 0) && (
                    <text class="ch-axis" x={xs[i]} y={height - 8} text-anchor="middle">
                      {monthShort(month.period)}
                    </text>
                  )}
                </g>
              ))}
              {index !== null && <line class="ch-cross" x1={xs[index]} x2={xs[index]} y1={padTop} y2={padTop + plotHeight} />}
            </svg>
            <div
              class="chart-hit"
              tabIndex={0}
              role="group"
              aria-label={t('chart.keys_hint')}
              style={{ insetInlineStart: `${padLeft}px`, insetInlineEnd: `${padRight}px` }}
              onKeyDown={(event) => {
                const next = keyStep(event.key, index ?? (event.key === 'End' ? count - 1 : 0), count);
                if (next !== null) {
                  event.preventDefault();
                  setIndex(next);
                } else if (event.key === 'Escape') setIndex(null);
              }}
              onFocus={() => index === null && setIndex(count - 1)}
              onBlur={() => setIndex(null)}
            />
            {focus && (
              <div class="chart-tip cash" style={{ left: `${tooltipLeft}%`, transform: tooltipLeft > 60 ? 'translateX(-100%)' : 'translateX(-8px)' }} aria-hidden="true">
                <strong>{formatPeriod(focus.period)}</strong>
                <span class="tip-row">
                  <span>{t('chart.cash')}</span>
                  <Ltr>{formatMinor(focus.cash.minor, currency, locale)}</Ltr>
                </span>
              </div>
            )}
          </>
        )}
      </div>
    </Figure>
  );
}

/** Stacked bars per month: entries posted by agents (blue, at the base) and by people (gold, on top). */
export function ActivityChart({ rows, id, title, summary }: { rows: { period: string; agent: number; human: number }[]; id: string; title: string; summary: string }) {
  const { ref, width } = useWidth(640);
  const [index, setIndex] = useState<number | null>(null);
  const count = rows.length;
  const height = 200;
  const padLeft = 40;
  const padRight = 8;
  const padTop = 12;
  const padBottom = 28;
  const plotWidth = Math.max(120, width - padLeft - padRight);
  const plotHeight = height - padTop - padBottom;
  const band = plotWidth / Math.max(count, 1);
  const barWidth = Math.min(24, band * 0.6);
  const top = Math.max(1, ...rows.map((row) => row.agent + row.human));
  const domain = { min: 0, max: top };
  const y = linear(domain, padTop + plotHeight, padTop);
  const focus = index !== null ? rows[index] : null;
  const tooltipLeft = index === null ? 0 : ((padLeft + band * (index + 0.5)) / width) * 100;
  const sum = (key: 'agent' | 'human') => rows.reduce((total, row) => total + row[key], 0);

  const legend = (
    <div class="chart-legend" aria-hidden="true">
      <span class="legend-item agent">
        <span class="swatch" />
        {t('chart.agent')} · {sum('agent')}
      </span>
      <span class="legend-item human">
        <span class="swatch" />
        {t('chart.person')} · {sum('human')}
      </span>
    </div>
  );

  const table = (
    <table>
      <caption>{title}</caption>
      <thead>
        <tr>
          <th scope="col">{t('chart.month')}</th>
          <th scope="col">{t('chart.agent')}</th>
          <th scope="col">{t('chart.person')}</th>
        </tr>
      </thead>
      <tbody>
        {rows.map((row) => (
          <tr key={row.period}>
            <th scope="row">{formatPeriod(row.period)}</th>
            <td>{row.agent}</td>
            <td>{row.human}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );

  return (
    <Figure id={id} title={title} summary={summary} legend={legend} table={table}>
      <div class="chart-body" ref={ref} dir="ltr">
        {count > 0 && (
          <>
            <svg
              class="chart-svg"
              viewBox={`0 0 ${width} ${height}`}
              width={width}
              height={height}
              role="img"
              aria-hidden="true"
              focusable="false"
              onPointerMove={(event) => setIndex(indexAt(event.clientX, (event.currentTarget as SVGElement).getBoundingClientRect(), padLeft, band, count))}
              onPointerLeave={() => setIndex(null)}
            >
              {ticks(domain, 3).map((value) => (
                <g key={value}>
                  <line class="ch-grid" x1={padLeft} x2={width - padRight} y1={y(value)} y2={y(value)} />
                  <text class="ch-axis" x={padLeft - 8} y={y(value)} text-anchor="end" dominant-baseline="middle">
                    {Math.round(value)}
                  </text>
                </g>
              ))}
              {rows.map((row, i) => {
                const cx = padLeft + band * (i + 0.5);
                const agentTop = y(row.agent);
                const humanTop = y(row.agent + row.human);
                return (
                  <g key={row.period} class={index === i ? 'focus' : ''}>
                    {row.agent > 0 && <rect class="ch-agent bar" x={cx - barWidth / 2} y={agentTop} width={barWidth} height={Math.max(1, y(0) - agentTop)} rx="2" />}
                    {row.human > 0 && (
                      <rect class="ch-human bar" x={cx - barWidth / 2} y={humanTop} width={barWidth} height={Math.max(1, agentTop - humanTop)} rx="2" />
                    )}
                    <text class="ch-axis" x={cx} y={height - 8} text-anchor="middle">
                      {monthShort(row.period)}
                    </text>
                  </g>
                );
              })}
            </svg>
            {focus && (
              <div class="chart-tip" style={{ left: `${tooltipLeft}%`, transform: tooltipLeft > 60 ? 'translateX(-100%)' : 'translateX(-8px)' }} aria-hidden="true">
                <strong>{formatPeriod(focus.period)}</strong>
                <span class="tip-row agent">
                  <span>{t('chart.agent')}</span>
                  <span>{focus.agent}</span>
                </span>
                <span class="tip-row human">
                  <span>{t('chart.person')}</span>
                  <span>{focus.human}</span>
                </span>
              </div>
            )}
          </>
        )}
      </div>
    </Figure>
  );
}

/** A tiny line with a soft fill, for the KPI cards. Decorative: the value beside it carries the meaning. */
export function Sparkline({ values, tone }: { values: bigint[]; tone: 'rev' | 'exp' | 'net' | 'cash' | 'neutral' }) {
  if (values.length < 2) return <span class="spark empty" aria-hidden="true" />;
  const numbers = values.map(Number);
  const min = Math.min(...numbers);
  const max = Math.max(...numbers);
  const span = max - min || 1;
  const step = 120 / (numbers.length - 1);
  const points = numbers.map((value, i) => [i * step, 34 - ((value - min) / span) * 30 - 2] as const);
  const line = points.map(([x, y], i) => `${i === 0 ? 'M' : 'L'}${x.toFixed(2)},${y.toFixed(2)}`).join(' ');
  const area = `${line} L120,36 L0,36 Z`;
  return (
    <svg class={`spark ${tone}`} viewBox="0 0 120 36" preserveAspectRatio="none" aria-hidden="true" focusable="false">
      <path class="spark-area" d={area} />
      <path class="spark-line" d={line} />
    </svg>
  );
}

/** Revenue or expenses split by a dimension value: one stacked bar and the legend that names each part. */
export function DimensionShares({
  rows,
  currency,
  id,
  title,
  summary,
}: {
  rows: { code: string; name: string; amount: bigint }[];
  currency: string;
  id: string;
  title: string;
  summary: string;
}) {
  const locale = intlLocale();
  const total = rows.reduce((sum, row) => sum + row.amount, 0n);
  const positive = rows.filter((row) => row.amount > 0n);
  const table = (
    <table>
      <caption>{title}</caption>
      <thead>
        <tr>
          <th scope="col">{t('chart.dimension_value')}</th>
          <th scope="col">{t('chart.amount')}</th>
        </tr>
      </thead>
      <tbody>
        {rows.map((row) => (
          <tr key={row.code}>
            <th scope="row">{row.name}</th>
            <td>{formatMinor(row.amount, currency, locale)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
  return (
    <Figure id={id} title={title} summary={summary} table={table}>
      {positive.length === 0 ? (
        <p class="muted chart-none">{t('chart.dimension_none')}</p>
      ) : (
        <>
          <div class="stack-bar" aria-hidden="true">
            {positive.map((row, i) => (
              <span key={row.code} class={`stack-seg c${i % 6}`} style={{ flexGrow: Number(row.amount) }} title={row.name} />
            ))}
          </div>
          <ul class="stack-legend">
            {positive.map((row, i) => (
              <li key={row.code}>
                <span class={`swatch c${i % 6}`} aria-hidden="true" />
                <span class="stack-name">{row.name}</span>
                <span class="stack-share">{total > 0n ? `${Number((row.amount * 1000n) / total) / 10}%` : '—'}</span>
                <Ltr class="stack-amount">{formatMinor(row.amount, currency, locale)}</Ltr>
              </li>
            ))}
          </ul>
        </>
      )}
    </Figure>
  );
}
