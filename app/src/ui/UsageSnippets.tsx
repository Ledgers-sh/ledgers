import { cliSnippet, mcpConfig } from '../lib/agentKeys';

/** The CLI and MCP snippets for a key, from the server URL. */
export function UsageSnippets({ baseUrl, token }: { baseUrl: string; token: string }) {
  return (
    <div class="snippets">
      <div>
        <h4>Command line</h4>
        <pre class="snippet mono" tabIndex={0} aria-label="Command line snippet">
          {cliSnippet(baseUrl, token)}
        </pre>
      </div>
      <div>
        <h4>MCP client config</h4>
        <pre class="snippet mono" tabIndex={0} aria-label="MCP client config">
          {mcpConfig(baseUrl, token)}
        </pre>
      </div>
    </div>
  );
}
