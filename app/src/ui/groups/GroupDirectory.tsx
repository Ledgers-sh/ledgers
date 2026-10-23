/**
 * The Group screens when the company the page is in belongs to no group. The
 * person may still see groups through their other companies, so each one is
 * listed with its logo and member companies, and "Open group" switches the
 * company to a member and shows that company's group view. The server decides
 * which groups and members the person sees; this lists what it returned.
 */
import { useState } from 'preact/hooks';
import type { GroupView } from '../../api/groups';
import type { CompanySummary } from '../../api/companies';
import { t } from '../../i18n';
import { companyHref } from '../../lib/companies';
import { resetScope } from '../../session';
import { Avatar, CompanyAvatar } from '../Avatar';
import { PageHead } from '../common';
import { Select } from '../Select';
import { Ltr } from '../../i18n/Ltr';
import { KindChip, RoleNotice } from './shared';

/** Switches to the group's first member company and opens its group view. */
export function openGroup(group: GroupView, pathname: string): void {
  const member = group.members[0];
  if (!member) return;
  resetScope();
  window.location.assign(companyHref(pathname, member.company_id, '#/group/view'));
}

export function GroupDirectory({
  groups,
  companies,
  pathname,
}: {
  groups: readonly GroupView[];
  companies: readonly CompanySummary[];
  pathname: string;
}) {
  const [picked, setPicked] = useState<string>('');
  const shown = picked === '' ? groups : groups.filter((group) => group.id === picked);
  return (
    <>
      <PageHead title={t('groups.title')}>{t('directory.intro')}</PageHead>
      <RoleNotice>{t('directory.not_here')}</RoleNotice>
      {groups.length > 1 && (
        <div class="row-actions group-view-controls">
          <div class="field">
            <span id="directory-group-label">{t('directory.select_group')}</span>
            <Select
              id="directory-group"
              labelledBy="directory-group-label"
              value={picked}
              options={[
                { value: '', label: t('directory.all_groups') },
                ...groups.map((group) => ({ value: group.id, label: group.name })),
              ]}
              onChange={(value) => setPicked(value)}
            />
          </div>
        </div>
      )}
      {groups.length === 0 && <RoleNotice>{t('directory.none')}</RoleNotice>}
      <div class="directory-list">
        {shown.map((group) => (
          <section class="panel group-card" key={group.id} aria-labelledby={`directory-${group.id}`}>
            <div class="panel-head group-card-head">
              <Avatar
                name={group.name}
                logo={group.logo_document_id ? { documentId: group.logo_document_id, companyId: group.logo_company_id ?? undefined } : null}
                size="lg"
              />
              <h2 id={`directory-${group.id}`} class="grow">
                <Ltr>{group.name}</Ltr>
              </h2>
              <button type="button" class="btn btn-primary" onClick={() => openGroup(group, pathname)}>
                {t('directory.open', { name: group.name })}
              </button>
            </div>
            <div class="panel-body">
              <ul class="group-card-members" aria-label={t('directory.members')}>
                {group.members.map((member) => {
                  const company = companies.find((item) => item.id === member.company_id);
                  return (
                    <li key={member.company_id}>
                      <CompanyAvatar companyId={member.company_id} name={member.company_name} size="sm" />
                      <span class="company-name">
                        <Ltr>{member.company_name}</Ltr>
                      </span>
                      <KindChip kind={member.kind} />
                      {company === undefined && <span class="muted">{t('directory.not_yours')}</span>}
                    </li>
                  );
                })}
              </ul>
            </div>
          </section>
        ))}
      </div>
    </>
  );
}
