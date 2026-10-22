import { useState } from 'preact/hooks';
import { AgentKeys } from './AgentKeys';
import { Icon } from './icons';
import { Empty, PageHead } from './common';
import { useSession } from '../session';
import { t, type MessageKey } from '../i18n';
import { CompanySettings } from './settings/CompanySettings';
import { YearSettings } from './settings/YearSettings';
import { NumberingSettings } from './settings/NumberingSettings';
import { TaxSettings } from './settings/TaxSettings';
import { ApprovalSettings } from './settings/ApprovalSettings';
import { InvoiceSettings } from './settings/InvoiceSettings';
import { AuditSettings } from './settings/AuditSettings';
import { People } from './People';
import { UnitsPanel } from './Units';

/** The Settings sections, in the order the left navigation lists them. */
export const SETTINGS_SECTIONS = [
  'company',
  'years',
  'numbering',
  'tax',
  'approvals',
  'invoices',
  'audit',
  'people',
  'units',
  'agent-keys',
] as const;

export type SettingsSection = (typeof SETTINGS_SECTIONS)[number];

const SECTION_LABEL: Record<SettingsSection, MessageKey> = {
  company: 'settings.company',
  years: 'settings.years',
  numbering: 'settings.numbering',
  tax: 'settings.tax',
  approvals: 'settings.approvals',
  invoices: 'settings.invoices',
  audit: 'settings.audit',
  people: 'settings.people',
  units: 'settings.units',
  'agent-keys': 'settings.agent_keys',
};

/** The section a `settings/...` route names; the company profile when the name is unknown or missing. */
export function settingsSectionOf(route: string): SettingsSection {
  const name = route.replace(/^settings\/?/, '');
  return (SETTINGS_SECTIONS as readonly string[]).includes(name) ? (name as SettingsSection) : 'company';
}

const SECTION_INTRO: Record<SettingsSection, MessageKey> = {
  company: 'settings.company_intro',
  years: 'settings.years_intro',
  numbering: 'settings.numbering_intro',
  tax: 'settings.tax_intro',
  approvals: 'settings.approvals_intro',
  invoices: 'settings.invoices_intro',
  audit: 'settings.audit_intro',
  people: 'settings.people_intro',
  units: 'units.intro',
  'agent-keys': 'agentKeys.intro',
};

/**
 * Settings: a left navigation of sections, each with its own address under
 * #/settings/<section>. Every section is a person's screen: an agent key gets
 * the explanation instead of forms that would fail.
 */
export function Settings({ section }: { section: SettingsSection }) {
  const { client } = useSession();
  const personal = !client.apiKey;
  const [creating, setCreating] = useState(false);
  const canCreate = section === 'agent-keys' && personal;

  return (
    <>
      <PageHead
        title={t('settings.title')}
        actions={
          canCreate ? (
            <button type="button" class="btn btn-primary" onClick={() => setCreating(true)}>
              <Icon name="plus" />
              {t('agentKeys.create')}
            </button>
          ) : undefined
        }
      >
        {t('settings.intro')}
      </PageHead>
      <div class="settings">
        <nav class="settings-nav" aria-label={t('settings.sections')}>
          <ul>
            {SETTINGS_SECTIONS.map((name) => (
              <li key={name}>
                <a
                  class="settings-link"
                  href={`#/settings/${name}`}
                  aria-current={name === section ? 'page' : undefined}
                >
                  {t(SECTION_LABEL[name])}
                </a>
              </li>
            ))}
          </ul>
        </nav>
        <div class="settings-body">
          <p class="muted settings-intro">{t(SECTION_INTRO[section])}</p>
          {!personal && section !== 'agent-keys' && section !== 'units' ? (
            <Empty title={t('settings.person_title')}>{t('settings.person_body')}</Empty>
          ) : section === 'company' ? (
            <CompanySettings />
          ) : section === 'years' ? (
            <YearSettings />
          ) : section === 'numbering' ? (
            <NumberingSettings />
          ) : section === 'tax' ? (
            <TaxSettings />
          ) : section === 'approvals' ? (
            <ApprovalSettings />
          ) : section === 'invoices' ? (
            <InvoiceSettings />
          ) : section === 'audit' ? (
            <AuditSettings />
          ) : section === 'people' ? (
            <People />
          ) : section === 'units' ? (
            <UnitsPanel />
          ) : (
            <AgentKeys creating={creating} onCreatingChange={setCreating} />
          )}
        </div>
      </div>
    </>
  );
}
