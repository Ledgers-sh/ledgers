import { useState } from 'preact/hooks';
import { AgentKeys } from './AgentKeys';
import { Icon } from './icons';
import { PageHead } from './common';
import { useSession } from '../session';
import { t } from '../i18n';

export type SettingsSection = 'agent-keys';

/**
 * Settings: one section for now, agent keys. Each section has its own address
 * under #/settings. Same shell as the other screens: a page head, the section
 * tabs, and panels.
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
      <nav class="tabs" aria-label={t('settings.sections')}>
        <a class="tab" href="#/settings/agent-keys" aria-current={section === 'agent-keys' ? 'page' : undefined}>
          {t('settings.agent_keys')}
        </a>
      </nav>
      {section === 'agent-keys' && <AgentKeys creating={creating} onCreatingChange={setCreating} />}
    </>
  );
}
