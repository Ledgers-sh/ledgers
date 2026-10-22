import { AgentKeys } from './AgentKeys';
import { PageHead } from './common';

export type SettingsSection = 'agent-keys';

/** Settings: one section for now, agent keys. Each section has its own address under #/settings. */
export function Settings({ section }: { section: SettingsSection }) {
  return (
    <>
      <PageHead title="Settings">Your account and the ledger's connections.</PageHead>
      <nav class="subnav" aria-label="Settings sections">
        <a href="#/settings/agent-keys" aria-current={section === 'agent-keys' ? 'page' : undefined}>
          Agent keys
        </a>
      </nav>
      {section === 'agent-keys' && <AgentKeys />}
    </>
  );
}
