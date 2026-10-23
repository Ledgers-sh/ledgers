/**
 * Intercompany reconciliation, per agreement between two companies: the
 * receivable against the payable and the income against the expense, read
 * from the books. Differences are marked in words and by a sign, and the open
 * month (the latest period not yet settled) is named on the pair.
 */
import { intercompanyReconciliation, type PairView, type PeriodStatus } from '../../api/groups';
import { useAsync } from '../../hooks';
import { formatPeriod, t } from '../../i18n';
import { openMonth, pairHasDifference } from '../../lib/groups';
import { ErrorBanner, Money, PageHead, Skeleton } from '../common';
import { Select } from '../Select';
import { useGroupContext } from './context';
import { CompanyName, GroupTabs, RoleNotice } from './shared';
import { useState } from 'preact/hooks';

export function ReconciliationPage() {
  const ctx = useGroupContext();
  const [pickedGroup, setPickedGroup] = useState<string | null>(null);
  const groups = ctx.groups ?? [];
  const groupId = pickedGroup ?? groups[0]?.id ?? '';
  const report = useAsync(
    () => (groupId === '' ? Promise.resolve(null) : intercompanyReconciliation(ctx.client, groupId)),
    [ctx.client.baseUrl, groupId],
  );

  return (
    <>
      <PageHead title={t('recon.title')}>{t('recon.intro')}</PageHead>
      <GroupTabs current="reconciliation" />
      {ctx.groups !== undefined && groups.length === 0 && <RoleNotice>{t('recon.no_group')}</RoleNotice>}
      {groups.length > 1 && (
        <div class="field group-pick">
          <span id="recon-group-label">{t('groups.pick_group')}</span>
          <Select
            id="recon-group"
            labelledBy="recon-group-label"
            value={groupId}
            options={groups.map((group) => ({ value: group.id, label: group.name }))}
            onChange={(value) => setPickedGroup(value)}
          />
        </div>
      )}
      {report.error !== undefined && <ErrorBanner error={report.error} onRetry={report.reload} />}
      {report.loading && report.data === undefined && <Skeleton rows={3} label={t('recon.loading')} />}
      {report.data && report.data.pairs.length === 0 && <p class="muted">{t('recon.no_pairs')}</p>}
      {report.data?.pairs.map((pair) => <PairCard key={pair.agreement_id} pair={pair} />)}
    </>
  );
}

function PairCard({ pair }: { pair: PairView }) {
  const difference = pairHasDifference(pair);
  const open = openMonth(pair);
  const headingId = `pair-${pair.agreement_id}`;
  return (
    <section class={`panel pair-card${difference ? ' has-difference' : ''}`} aria-labelledby={headingId}>
      <div class="panel-head">
        <h2 id={headingId} class="pair-title">
          <CompanyName name={pair.licensor.name} />
          <span aria-hidden="true"> ↔ </span>
          <CompanyName name={pair.licensee.name} />
        </h2>
        <span class={`chip ${difference ? 'status-rejected' : 'status-approved'}`}>
          {difference ? t('recon.difference') : t('recon.balanced')}
        </span>
        {open ? (
          <span class="chip open-chip" data-open-month={open}>
            {t('recon.open_month', { period: formatPeriod(open) })}
          </span>
        ) : (
          <span class="chip status-approved">{t('recon.all_settled')}</span>
        )}
      </div>
      <div class="panel-body">
        <dl class="recon-figures">
          <Figure label={t('recon.receivable')} minor={pair.receivable_minor} currency={pair.currency} />
          <Figure label={t('recon.payable')} minor={pair.payable_minor} currency={pair.currency} />
          <Figure
            label={t('recon.balance_difference')}
            minor={pair.balance_difference_minor}
            currency={pair.currency}
            flagged={pair.balance_difference_minor !== 0}
          />
          <Figure label={t('recon.income')} minor={pair.income_minor} currency={pair.currency} />
          <Figure label={t('recon.expense')} minor={pair.expense_minor} currency={pair.currency} />
          <Figure
            label={t('recon.income_difference')}
            minor={pair.income_difference_minor}
            currency={pair.currency}
            flagged={pair.income_difference_minor !== 0}
          />
        </dl>
        <div class="table-wrap">
          <table class="recon-periods">
            <caption>{t('recon.periods')}</caption>
            <thead>
              <tr>
                <th scope="col">{t('recon.period')}</th>
                <th scope="col">{t('agreements.licensor')}</th>
                <th scope="col">{t('agreements.licensee')}</th>
                <th scope="col" class="num">
                  {t('recon.gross')}
                </th>
                <th scope="col" class="num">
                  {t('recon.wht')}
                </th>
              </tr>
            </thead>
            <tbody>
              {pair.periods.map((row) => (
                <PeriodRow key={row.period} row={row} currency={pair.currency} open={open === row.period} />
              ))}
            </tbody>
          </table>
        </div>
      </div>
    </section>
  );
}

function Figure({ label, minor, currency, flagged }: { label: string; minor: number; currency: string; flagged?: boolean }) {
  return (
    <div class={`recon-figure${flagged ? ' flagged' : ''}`}>
      <dt>{label}</dt>
      <dd>
        {flagged && <span class="flag-tag">{t('recon.differs')}</span>}
        <Money minor={String(minor)} currency={currency} />
      </dd>
    </div>
  );
}

function PeriodRow({ row, currency, open }: { row: PeriodStatus; currency: string; open: boolean }) {
  return (
    <tr class={open ? 'open-month' : undefined}>
      <th scope="row">
        {formatPeriod(row.period)}
        {open && <span class="chip open-chip">{t('recon.open')}</span>}
      </th>
      <td>{t(`agreements.doc.${row.licensor_status}`)}</td>
      <td>{t(`agreements.doc.${row.licensee_status}`)}</td>
      <td class="num">
        <Money minor={String(row.gross_minor)} currency={currency} />
      </td>
      <td class="num">
        <Money minor={String(row.wht_minor)} currency={currency} />
      </td>
    </tr>
  );
}
