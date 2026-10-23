/**
 * Intercompany agreements in the company's books: each row is the company's
 * own side, with the two parties, the basis and rate, the withholding and the
 * status. A person who may change settings starts a new agreement here.
 */
import { agreementsList, type AgreementKind, type AgreementView, type GroupView } from '../../api/groups';
import { useAsync } from '../../hooks';
import { basisText, canChangeSettings, formatRateBp, whtDisplay } from '../../lib/groups';
import { intlLocale, t, type MessageKey } from '../../i18n';
import { ErrorBanner, Empty, Money, PageHead, Skeleton } from '../common';
import { useGroupContext, type GroupContext } from './context';
import { CompanyName, GroupTabs, RoleNotice } from './shared';

const KIND_KEY: Record<AgreementKind, MessageKey> = {
  licence: 'agreements.kind.licence',
  royalty: 'agreements.kind.royalty',
  management_fee: 'agreements.kind.management_fee',
  loan: 'agreements.kind.loan',
};

/**
 * The two parties of an agreement, by name. The company the person may not
 * see is null: it is shown as hidden, never named.
 */
export function partiesOf(agreement: AgreementView, group: GroupView | undefined, ctx: GroupContext) {
  const counterparty = agreement.counterparty_id;
  const counterpartyName = counterparty
    ? (group?.members.find((member) => member.company_id === counterparty)?.company_name ?? null)
    : null;
  const ownName = ctx.nameOf(ctx.currentCompany);
  return {
    licensor: agreement.side === 'licensor' ? ownName : counterpartyName,
    licensee: agreement.side === 'licensee' ? ownName : counterpartyName,
  };
}

export function AgreementsPage() {
  const ctx = useGroupContext();
  const list = useAsync(() => agreementsList(ctx.client), [ctx.client.baseUrl, ctx.currentCompany]);
  const canCreate = canChangeSettings(ctx.role) && (ctx.groups?.length ?? 0) > 0;

  return (
    <>
      <PageHead
        title={t('agreements.title')}
        actions={
          canCreate ? (
            <a class="btn btn-primary" href="#/group/agreements/new">
              {t('agreements.new')}
            </a>
          ) : undefined
        }
      >
        {t('agreements.intro')}
      </PageHead>
      <GroupTabs current="agreements" />
      {!canCreate && ctx.groups !== undefined && (
        <RoleNotice>{(ctx.groups?.length ?? 0) === 0 ? t('agreements.needs_group') : t('agreements.needs_settings')}</RoleNotice>
      )}
      {list.error !== undefined && <ErrorBanner error={list.error} onRetry={list.reload} />}
      {list.loading && list.data === undefined && <Skeleton rows={3} label={t('agreements.loading')} />}
      {list.data && list.data.length === 0 && (
        <Empty title={t('agreements.empty_title')}>
          <p>{t('agreements.empty_body')}</p>
        </Empty>
      )}
      {list.data && list.data.length > 0 && (
        <div class="panel">
          <div class="table-wrap">
            <table class="agreements-table">
              <caption class="sr-only">{t('agreements.title')}</caption>
              <thead>
                <tr>
                  <th scope="col">{t('agreements.col.kind')}</th>
                  <th scope="col">{t('agreements.col.parties')}</th>
                  <th scope="col">{t('agreements.col.basis')}</th>
                  <th scope="col" class="num">
                    {t('agreements.col.rate')}
                  </th>
                  <th scope="col" class="num">
                    {t('agreements.col.wht')}
                  </th>
                  <th scope="col">{t('agreements.col.status')}</th>
                </tr>
              </thead>
              <tbody>
                {list.data.map((agreement) => (
                  <AgreementRow key={`${agreement.id}-${agreement.side}`} agreement={agreement} ctx={ctx} groups={ctx.groups} />
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}
    </>
  );
}

function AgreementRow({
  agreement,
  ctx,
  groups,
}: {
  agreement: AgreementView;
  ctx: GroupContext;
  groups: GroupView[] | undefined;
}) {
  const group = groups?.find((item) => item.id === agreement.group_id);
  const parties = partiesOf(agreement, group, ctx);
  const wht = whtDisplay(agreement);
  const hiddenLicensor = parties.licensor === null;
  const hiddenLicensee = parties.licensee === null;
  const terminated = agreement.terminated_at !== null;
  return (
    <tr>
      <td>
        <a href={`#/group/agreements/${encodeURIComponent(agreement.id)}`}>{t(KIND_KEY[agreement.kind])}</a>
      </td>
      <td>
        <div class="parties">
          <span class="party">
            <span class="party-role muted">{t('agreements.licensor')}</span>
            <CompanyName name={parties.licensor} hidden={hiddenLicensor} />
          </span>
          <span class="party">
            <span class="party-role muted">{t('agreements.licensee')}</span>
            <CompanyName name={parties.licensee} hidden={hiddenLicensee} />
          </span>
        </div>
      </td>
      <td>{t(basisText(agreement.basis))}</td>
      <td class="num">
        {agreement.basis === 'fixed_monthly' && agreement.fixed_minor !== null ? (
          <Money minor={String(agreement.fixed_minor)} currency={agreement.currency} />
        ) : (
          formatRateBp(agreement.rate_bp, intlLocale())
        )}
      </td>
      <td class="num">
        {agreement.kind === 'licence' || agreement.kind === 'royalty' ? (
          <>
            <span>{formatRateBp(agreement.wht_bp, intlLocale())}</span>
            <span class="hint-text muted">{t(wht.key)}</span>
          </>
        ) : (
          <span class="muted">{t('groups.wht.not_applicable')}</span>
        )}
      </td>
      <td>
        <span class={`chip ${terminated ? 'status-rejected' : 'status-active'}`}>
          {terminated ? t('agreements.terminated') : t('agreements.active')}
        </span>
      </td>
    </tr>
  );
}
