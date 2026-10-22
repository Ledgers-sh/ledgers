import { useState } from 'preact/hooks';
import { keysList, keysRevoke } from '../api/commands';
import type { AgentKeyView, KeysCreateOutput } from '../api/types';
import { useAsync } from '../hooks';
import { TOKEN_PLACEHOLDER, tierLabel } from '../lib/agentKeys';
import { useSession } from '../session';
import { ConfirmDialog, Empty, ErrorBanner, LOCALE, Skeleton } from './common';
import { Icon } from './icons';
import { AgentKeyCreate, ShowOnceDialog } from './AgentKeyCreate';
import { UsageSnippets } from './UsageSnippets';

/** A timestamp as the person's locale shows it, or a dash when there is none. */
export function formatWhen(iso: string | null): string {
  if (!iso) return '—';
  const when = new Date(iso);
  return Number.isNaN(when.getTime()) ? iso : when.toLocaleString(LOCALE, { dateStyle: 'medium', timeStyle: 'short' });
}

/** "EUR 1000.00 / 5000.00" for each currency: per entry, then per day. */
export function limitsSummary(key: AgentKeyView): string[] {
  const entries = Object.entries(key.limits);
  if (entries.length === 0) return ['No currencies: cannot move money'];
  return entries.map(([currency, limit]) => `${currency} ${limit.per_entry} / ${limit.per_day}`);
}

/**
 * Settings → Agent keys. Person sessions list, create and revoke keys. An
 * agent key cannot manage keys, so this screen says so instead of calling the API.
 */
export function AgentKeys() {
  const { client, toast } = useSession();
  const personal = !client.apiKey;
  const state = useAsync(() => keysList(client), [client.baseUrl, client.apiKey]);
  const [creating, setCreating] = useState(false);
  const [created, setCreated] = useState<KeysCreateOutput | null>(null);
  const [revoking, setRevoking] = useState<AgentKeyView | null>(null);
  const [revokeBusy, setRevokeBusy] = useState(false);
  const [revokeError, setRevokeError] = useState<unknown>(null);

  if (!personal) {
    return (
      <section class="settings-section" aria-labelledby="agent-keys-title">
        <h2 id="agent-keys-title">Agent keys</h2>
        <Empty title="Sign in as a person to manage agent keys">
          This session uses an agent key. An agent key cannot create, list or revoke agent keys. Sign in with a passkey to
          manage them here.
        </Empty>
      </section>
    );
  }

  const confirmRevoke = async () => {
    if (!revoking) return;
    setRevokeBusy(true);
    setRevokeError(null);
    try {
      await keysRevoke(client, revoking.id);
      toast(`Revoked ${revoking.name}. It stops working on its next request.`);
      setRevoking(null);
      state.reload();
    } catch (error) {
      setRevokeError(error);
    } finally {
      setRevokeBusy(false);
    }
  };

  return (
    <section class="settings-section" aria-labelledby="agent-keys-title">
      <div class="section-head">
        <div>
          <h2 id="agent-keys-title">Agent keys</h2>
          <p class="muted">
            A key lets an agent or a script use this ledger within its tier and limits. Revoke a key to stop it at once. Its
            secret is shown only when you create it.
          </p>
        </div>
        <button type="button" class="btn btn-primary" onClick={() => setCreating(true)}>
          <Icon name="plus" />
          Create key
        </button>
      </div>

      {state.loading && !state.data && <Skeleton rows={3} label="Loading agent keys" />}
      {state.error && !state.data && <ErrorBanner error={state.error} onRetry={state.reload} />}
      {state.data &&
        (state.data.keys.length === 0 ? (
          <Empty title="No agent keys yet">Create a key to let an agent or a script use this ledger.</Empty>
        ) : (
          <div class="panel">
            <div class="table-wrap">
              <table class="ledger cards">
                <thead>
                  <tr>
                    <th scope="col">Name</th>
                    <th scope="col">Key</th>
                    <th scope="col">Tier</th>
                    <th scope="col">Limits (entry / day)</th>
                    <th scope="col">Last used</th>
                    <th scope="col">Status</th>
                    <th scope="col">
                      <span class="sr-only">Actions</span>
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {state.data.keys.map((key) => (
                    <tr key={key.id}>
                      <td class="main" data-label="Name">
                        {key.name}
                        <div class="muted small">Created {formatWhen(key.created_at)}</div>
                      </td>
                      <td class="mono" data-label="Key">
                        {key.prefix ?? '—'}
                      </td>
                      <td data-label="Tier">{tierLabel(key.tier)}</td>
                      <td class="num" data-label="Limits">
                        {limitsSummary(key).map((line) => (
                          <div key={line}>{line}</div>
                        ))}
                      </td>
                      <td data-label="Last used">{key.last_used_at ? formatWhen(key.last_used_at) : <span class="muted">Never</span>}</td>
                      <td data-label="Status">
                        {key.revoked_at ? (
                          <span class="status revoked">Revoked {formatWhen(key.revoked_at)}</span>
                        ) : (
                          <span class="status active">Active</span>
                        )}
                      </td>
                      <td class="actions">
                        {!key.revoked_at && (
                          <button
                            type="button"
                            class="btn btn-sm btn-danger"
                            onClick={() => {
                              setRevokeError(null);
                              setRevoking(key);
                            }}
                          >
                            Revoke<span class="sr-only"> {key.name}</span>
                          </button>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </div>
        ))}

      <div class="panel usage">
        <div class="panel-body">
          <h3>Use a key</h3>
          <p class="muted">
            Replace <code>{TOKEN_PLACEHOLDER}</code> with a key's secret, the one shown when you created it. Keys cannot be
            shown again.
          </p>
          <UsageSnippets baseUrl={client.baseUrl} token={TOKEN_PLACEHOLDER} />
        </div>
      </div>

      <AgentKeyCreate
        open={creating}
        onClose={() => setCreating(false)}
        onCreated={(output) => {
          setCreating(false);
          setCreated(output);
          state.reload();
        }}
      />
      <ShowOnceDialog output={created} baseUrl={client.baseUrl} onClose={() => setCreated(null)} />
      <ConfirmDialog
        open={revoking !== null}
        onClose={() => setRevoking(null)}
        title={`Revoke ${revoking?.name ?? 'this key'}?`}
        confirmLabel="Revoke key"
        danger
        busy={revokeBusy}
        error={revokeError}
        onConfirm={() => void confirmRevoke()}
      >
        <p>
          <span class="mono">{revoking?.prefix}</span> stops working on its next request, over HTTP and MCP. The record
          stays in the list, marked revoked. This cannot be undone: a new key needs a new secret.
        </p>
      </ConfirmDialog>
    </section>
  );
}
