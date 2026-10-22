import { cliSnippet, mcpConfig } from '../lib/agentKeys';
import { t } from '../i18n';

/** The CLI and MCP snippets for a key, from the server URL. The code itself is the same in every language. */
export function UsageSnippets({ baseUrl, token }: { baseUrl: string; token: string }) {
  return (
    <div class="snippets">
      <div>
        <h4>{t('agentKeys.cli_heading')}</h4>
        <pre class="snippet mono" tabIndex={0} aria-label={t('agentKeys.cli_label')}>
          {cliSnippet(baseUrl, token)}
        </pre>
      </div>
      <div>
        <h4>{t('agentKeys.mcp_heading')}</h4>
        <pre class="snippet mono" tabIndex={0} aria-label={t('agentKeys.mcp_label')}>
          {mcpConfig(baseUrl, token)}
        </pre>
      </div>
    </div>
  );
}
