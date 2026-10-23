/**
 * The group view: each company the person may see, side by side, with its
 * P&L for the month and its balance sheet at the month's end. Combined
 * consolidation is a Ledgers Cloud feature: the screen says so and offers
 * nothing to press.
 */
import { useState } from 'preact/hooks';
import type { Unit } from '../../api/types';
import { groupView, type GroupViewOutput } from '../../api/groups';
import { reportsUnitsSummary, unitsList } from '../../api/commands';
import { Avatar } from '../Avatar';
import { Money as MoneyView } from '../common';
import { useAsync } from '../../hooks';
import { formatPeriod, t } from '../../i18n';
import { ErrorBanner, Money, PageHead, Skeleton } from '../common';
import { Ltr } from '../../i18n/Ltr';
import { PeriodPicker, rangeLabel } from '../PeriodPicker';
import { Select } from '../Select';
import { useGroupContext } from './context';
import { CompanyName, GroupTabs, KindChip, RoleNotice } from './shared';
import type { UnitsSummaryOutput } from '../../api/types';
import type { MonthRange } from '../../lib/trends';
import { formatBpPercent, monthEnd } from '../../lib/groups';
import { intlLocale } from '../../i18n';

export function GroupViewPage() {
  const ctx = useGroupContext();
  const groups = ctx.groups ?? [];
  const [pickedGroup, setPickedGroup] = useState<string | null>(null);
  const [range, setRange] = useState<MonthRange>(() => {
    const month = new Date().toISOString().slice(0, 7);
    return { from: month, to: month };
  });
  const groupId = pickedGroup ?? groups[0]?.id ?? '';
  const period = range.to;
  const report = useAsync(
    () => (groupId === '' ? Promise.resolve(null) : groupView(ctx.client, groupId, period, monthEnd(period))),
    [ctx.client.baseUrl, groupId, period],
  );

  return (
    <>
      <PageHead title={t('view.title')}>{t('view.intro')}</PageHead>
      <GroupTabs current="view" />
      {ctx.groups !== undefined && groups.length === 0 && <RoleNotice>{t('recon.no_group')}</RoleNotice>}
      <div class="row-actions group-view-controls">
        {groups.length > 1 && (
          <div class="field">
            <span id="view-group-label">{t('groups.pick_group')}</span>
            <Select
              id="view-group"
              labelledBy="view-group-label"
              value={groupId}
              options={groups.map((group) => ({ value: group.id, label: group.name }))}
              onChange={(value) => setPickedGroup(value)}
            />
          </div>
        )}
        <div class="field">
          <span id="view-period-label">{t('view.period')}</span>
          <PeriodPicker range={range} onChange={setRange} />
        </div>
        <span class="muted">{rangeLabel(range)}</span>
      </div>
      {report.error !== undefined && <ErrorBanner error={report.error} onRetry={report.reload} />}
      {report.loading && report.data === undefined && <Skeleton rows={3} label={t('view.loading')} />}
      {report.data && <GroupFigures view={report.data} />}
    </>
  );
}

function GroupFigures({ view }: { view: GroupViewOutput }) {
  const locale = intlLocale();
  return (
    <>
      <div class="consolidation-note panel">
        <div class="panel-body">
          <h2>{t('view.consolidated_title')}</h2>
          <p>{t('view.consolidated_body')}</p>
        </div>
      </div>
      <p class="muted">
        {t('view.figures_for', { period: formatPeriod(view.period), asOf: view.as_of })}
      </p>
      <div class="view-columns">
        {view.members.map((member) => (
          <section class="panel member-column" key={member.company.id} aria-labelledby={`member-${member.company.id}`}>
            <div class="panel-head">
              <h2 id={`member-${member.company.id}`} class="grow">
                <CompanyName name={member.company.name} companyId={member.company.id} />
              </h2>
              <KindChip kind={member.kind} />
            </div>
            <div class="panel-body">
              <p class="muted">
                {t('view.ownership', { percent: formatBpPercent(member.ownership_bp, locale) })}
              </p>
              <dl class="figures">
                <Figure label={t('view.revenue')} minor={member.revenue_minor} currency={member.currency} />
                <Figure label={t('view.expenses')} minor={member.expenses_minor} currency={member.currency} />
                <Figure label={t('view.net_income')} minor={member.net_income_minor} currency={member.currency} />
                <Figure label={t('view.assets')} minor={member.assets_minor} currency={member.currency} />
                <Figure label={t('view.liabilities')} minor={member.liabilities_minor} currency={member.currency} />
                <Figure label={t('view.equity')} minor={member.equity_minor} currency={member.currency} />
              </dl>
              <p class={member.balanced ? 'chip status-approved' : 'chip status-rejected'}>
                {member.balanced ? t('view.balanced') : t('view.unbalanced')}
              </p>
              <h3 class="member-units-heading">{t('view.business_units')}</h3>
              <MemberUnits companyId={member.company.id} period={view.period} />
            </div>
          </section>
        ))}
      </div>
    </>
  );
}

function Figure({ label, minor, currency }: { label: string; minor: number; currency: string }) {
  return (
    <div class="figure">
      <dt>{label}</dt>
      <dd>
        <Money minor={String(minor)} currency={currency} />
      </dd>
    </div>
  );
}

/**
 * A member company's business units, each with its logo and its month figures
 * from reports.units_summary. Read with the member as the company, so the
 * server's own checks apply to that company's units.
 */
function MemberUnits({ companyId, period }: { companyId: string; period: string }) {
  const ctx = useGroupContext();
  const config = { ...ctx.client, company: companyId };
  const data = useAsync(
    () => Promise.all([unitsList(config), reportsUnitsSummary(config, period)]),
    [ctx.client.baseUrl, companyId, period],
  );
  if (data.error !== undefined) return <p class="muted">{t('view.units_failed')}</p>;
  if (data.data === undefined) return <Skeleton rows={2} />;
  const [list, summary] = data.data;
  const units = list.units.filter((unit) => !unit.archived);
  if (units.length === 0) return <p class="muted">{t('view.units_none')}</p>;
  return <UnitFigures companyId={companyId} units={units} summary={summary} />;
}

function UnitFigures({
  companyId,
  units,
  summary,
}: {
  companyId: string;
  units: Unit[];
  summary: UnitsSummaryOutput;
}) {
  return (
    <ul class="unit-figures" aria-label={t('view.business_units')}>
      {units.map((unit) => {
        const row = summary.units.find((item) => item.unit === unit.code);
        return (
          <li class="unit-figure" key={unit.code}>
            <span class="unit-figure-name">
              <Avatar
                name={unit.name}
                colour={unit.colour}
                logo={unit.logo_document_id ? { documentId: unit.logo_document_id, companyId } : null}
                size="sm"
              />
              <span>
                <Ltr>{unit.name}</Ltr> <span class="muted">{unit.code}</span>
              </span>
            </span>
            {row ? (
              <dl class="unit-figure-values">
                <div>
                  <dt>{t('view.revenue')}</dt>
                  <dd>
                    <MoneyView minor={row.revenue.minor} currency={row.revenue.currency} />
                  </dd>
                </div>
                <div>
                  <dt>{t('view.expenses')}</dt>
                  <dd>
                    <MoneyView minor={row.expenses.minor} currency={row.expenses.currency} />
                  </dd>
                </div>
                <div>
                  <dt>{t('view.net_income')}</dt>
                  <dd>
                    <MoneyView minor={row.result.minor} currency={row.result.currency} />
                  </dd>
                </div>
              </dl>
            ) : null}
          </li>
        );
      })}
    </ul>
  );
}
