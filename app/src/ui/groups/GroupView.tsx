/**
 * The group view: each company the person may see, side by side, with its
 * P&L for the month and its balance sheet at the month's end. Combined
 * consolidation is a Ledgers Cloud feature: the screen says so and offers
 * nothing to press.
 */
import { useState } from 'preact/hooks';
import { groupView, type GroupViewOutput } from '../../api/groups';
import { useAsync } from '../../hooks';
import { formatPeriod, t } from '../../i18n';
import { ErrorBanner, Money, PageHead, Skeleton } from '../common';
import { PeriodPicker, rangeLabel } from '../PeriodPicker';
import { Select } from '../Select';
import { useGroupContext } from './context';
import { CompanyName, GroupTabs, KindChip, RoleNotice } from './shared';
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
                <CompanyName name={member.company.name} />
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
