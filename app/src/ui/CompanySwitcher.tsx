/**
 * The company switcher in the top bar, left of the business-unit switcher. It
 * names the company every request acts for and the role the person holds in
 * each. The choice is written to the URL (`?company=`) and the page reloads
 * into it, with the unit scope and the period back at their defaults. Hidden
 * when the person belongs to one company: the create dialog is in Settings.
 */
import type { CompanySummary } from '../api/companies';
import { t, type MessageKey } from '../i18n';
import { companyHref, roleBadgeClass } from '../lib/companies';
import { resetScope } from '../session';
import { Select, type SelectOption } from './Select';

export function CompanySwitcher({
  companies,
  value,
  pathname,
}: {
  companies: readonly CompanySummary[];
  value: string;
  pathname: string;
}) {
  if (companies.length <= 1) return null;
  const options: SelectOption[] = companies.map((company) => ({
    value: company.id,
    label: company.name,
    badge: { label: t(`people.role.${company.role}` as MessageKey), className: roleBadgeClass(company.role) },
  }));
  return (
    <div class="company-switch">
      <span class="company-switch-label" id="company-switch-label">
        {t('companies.switch_label')}
      </span>
      <Select
        id="company-switch"
        labelledBy="company-switch-label"
        class="compact"
        value={value}
        options={options}
        searchable={false}
        onChange={(next) => {
          if (next === value) return;
          // The period and the unit scope start again in the other company.
          resetScope();
          window.location.assign(companyHref(pathname, next));
        }}
      />
    </div>
  );
}
