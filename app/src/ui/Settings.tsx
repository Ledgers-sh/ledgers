import { AgentKeys } from './AgentKeys';
import { PageHead } from './common';
import { t } from '../i18n';

export type SettingsSection = 'agent-keys';

/** Settings: one section for now, agent keys. Each section has its own address under #/settings. */
export function Settings({ section }: { section: SettingsSection }) {
  return (
    <>
      <PageHead title={t('settings.title')}>{t('settings.intro')}</PageHead>
      <nav class="subnav" aria-label={t('settings.sections')}>
        <a href="#/settings/agent-keys" aria-current={section === 'agent-keys' ? 'page' : undefined}>
          {t('settings.agent_keys')}
        </a>
      </nav>
      {section === 'agent-keys' && <AgentKeys />}
    </>
  );
}
