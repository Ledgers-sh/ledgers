/**
 * "By business unit" on the Overview of the whole company: one card per unit
 * with its revenue, expenses, result and share of company revenue, and the
 * eliminations row that explains why the units do not add up to the total.
 * A card is a link that switches the Overview to that unit.
 */
import type { ComponentChildren } from 'preact';
import type { Amount, Unit, UnitSummaryRow, UnitsSummaryOutput } from '../api/types';
import { formatPeriod, intlLocale, t } from '../i18n';
import { Ltr } from '../i18n/Ltr';
import { formatMinor } from '../lib/money';
import { colourOf, hashWithUnit, shareBps } from '../lib/units';
import { Avatar } from './Avatar';
import { ErrorBanner, Skeleton } from './common';

function Figure({ label, amount }: { label: string; amount: Amount }) {
  const locale = intlLocale();
  const negative = amount.minor.startsWith('-');
  return (
    <span class="unit-figure">
      <span class="kpi-label">{label}</span>
      <span class={`num${negative ? ' neg' : ''}`}>
        <Ltr>{formatMinor(amount.minor, amount.currency, locale)}</Ltr>
      </span>
    </span>
  );
}

export function UnitsComparison({
  units,
  summary,
  loading,
  error,
  onRetry,
}: {
  units: readonly Unit[];
  summary: UnitsSummaryOutput | null;
  loading: boolean;
  error: unknown;
  onRetry: () => void;
}): ComponentChildren {
  const heading = (
    <div class="panel-head">
      <h2 id="units-summary-title" class="grow">
        {t('units.summary_title')}
      </h2>
    </div>
  );
  if (error !== undefined && error !== null && !summary) {
    return (
      <section class="panel units-summary" aria-labelledby="units-summary-title">
        {heading}
        <div class="panel-body">
          <ErrorBanner error={error} onRetry={onRetry} />
        </div>
      </section>
    );
  }
  if (!summary) {
    return loading ? <Skeleton rows={2} label={t('units.summary_loading')} /> : null;
  }

  const currency = summary.consolidated.revenue.currency;
  const byCode = new Map(units.map((unit) => [unit.code, unit]));
  const total = BigInt(summary.consolidated.revenue.minor);
  const rows: UnitSummaryRow[] = summary.units.filter((row) => row.unit && byCode.has(row.unit));
  const elim = summary.eliminations;

  return (
    <section class="panel units-summary" aria-labelledby="units-summary-title">
      {heading}
      <div class="panel-body units-body">
        <p class="muted">{t('units.summary_intro', { period: formatPeriod(summary.period) })}</p>
        <ul class="unit-cards" aria-label={t('units.summary_list')}>
          {rows.map((row) => {
            const unit = byCode.get(row.unit as string) as Unit;
            const colour = colourOf(unit);
            // A unit's share of company revenue: its external revenue (recharges excluded) over the company's.
            // Recharge income is in the unit's own books but not in company revenue, so it is not counted here.
            const share = shareBps(BigInt(row.external_revenue.minor), total);
            const shareText = new Intl.NumberFormat(intlLocale(), { style: 'percent', maximumFractionDigits: 1 }).format(share / 10_000);
            const label = t('units.card_label', {
              name: unit.name,
              revenue: formatMinor(row.revenue.minor, currency, intlLocale()),
              expenses: formatMinor(row.expenses.minor, currency, intlLocale()),
              result: formatMinor(row.result.minor, currency, intlLocale()),
            });
            return (
              <li key={unit.code}>
                <a class="unit-card" href={`#/${hashWithUnit('overview', unit.code)}`} aria-label={`${unit.code}: ${unit.name}. ${label}`}>
                  <span class="unit-card-head">
                    {unit.logo_document_id ? (
                      <Avatar name={unit.name} colour={colour} logo={{ documentId: unit.logo_document_id }} size="md" />
                    ) : (
                      <>
                        <span class="unit-dot" style={{ background: colour }} aria-hidden="true" />
                        <span class="mono unit-card-code">{unit.code}</span>
                      </>
                    )}
                    <span class="unit-card-name">{unit.name}</span>
                  </span>
                  <span class="unit-card-figures">
                    <Figure label={t('units.revenue')} amount={row.revenue} />
                    <Figure label={t('units.expenses')} amount={row.expenses} />
                    <Figure label={t('units.result')} amount={row.result} />
                  </span>
                  <span class="unit-share">
                    <span class="unit-share-label">
                      {t('units.share_of_revenue')} <span class="num">{shareText}</span>
                    </span>
                    <span class="unit-share-bar" aria-hidden="true">
                      <span style={{ width: `${Math.min(100, share / 100)}%`, background: colour }} />
                    </span>
                  </span>
                </a>
              </li>
            );
          })}
        </ul>
        <div class="unit-elim" role="note">
          <strong>{t('units.eliminations')}</strong>
          <span class="unit-elim-figures">
            <Figure label={t('units.revenue')} amount={elim.revenue} />
            <Figure label={t('units.expenses')} amount={elim.expenses} />
          </span>
          <span class="muted unit-elim-row-label">{t('units.eliminations_row')}</span>
        </div>
      </div>
    </section>
  );
}
