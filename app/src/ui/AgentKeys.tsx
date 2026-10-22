import { useState } from 'preact/hooks';
import { keysList, keysRevoke } from '../api/commands';
import type { AgentKeyView, KeysCreateOutput } from '../api/types';
import { useAsync } from '../hooks';
import { TOKEN_PLACEHOLDER, tierLabelKey } from '../lib/agentKeys';
import { useSession } from '../session';
import { intlLocale, t, tRich, type MessageKey } from '../i18n';
import { Ltr } from '../i18n/Ltr';
import { ConfirmDialog, Empty, ErrorBanner, Skeleton } from './common';
import { AgentKeyCreate, ShowOnceDialog } from './AgentKeyCreate';
import { UsageSnippets } from './UsageSnippets';

/** A timestamp as the person's locale shows it, or a dash when there is none. */
export function formatWhen(iso: string | null): string {
  if (!iso) return t('common.empty_cell');
  const when = new Date(iso);
  return Number.isNaN(when.getTime())
    ? iso
    : when.toLocaleString(intlLocale(), { dateStyle: 'medium', timeStyle: 'short' });
}

/** "EUR 1000.00 / 5000.00" for each currency: per entry, then per day. */
export function limitsSummary(key: AgentKeyView): string[] {
  const entries = Object.entries(key.limits);
  if (entries.length === 0) return [t('agentKeys.no_currencies')];
  return entries.map(([currency, limit]) => `${currency} ${limit.per_entry} / ${limit.per_day}`);
}

/** A key's tier, in the reader's language; the server's own word for a tier the app does not know. */
function tierText(tier: string): string {
  const key: MessageKey | null = tierLabelKey(tier);
  return key ? t(key) : tier;
}

/**
 * Settings → Agent keys. Person sessions list, create and revoke keys. An
 * agent key cannot manage keys, so this screen says so instead of calling the API.
 * The create button is in the page head (Settings), which owns `creating`.
 */
export function AgentKeys({ creating, onCreatingChange }: { creating: boolean; onCreatingChange: (open: boolean) => void }) {
  const { client, toast } = useSession();
  const personal = !client.apiKey;
  const state = useAsync(() => keysList(client), [client.baseUrl, client.apiKey]);
  const [created, setCreated] = useState<KeysCreateOutput | null>(null);
  const [revoking, setRevoking] = useState<AgentKeyView | null>(null);
  const [revokeBusy, setRevokeBusy] = useState(false);
  const [revokeError, setRevokeError] = useState<unknown>(null);

  if (!personal) {
    return (
      <section class="panel" aria-labelledby="agent-keys-title">
        <div class="panel-head">
          <h2 id="agent-keys-title">{t('agentKeys.title')}</h2>
        </div>
        <div class="panel-body">
          <Empty title={t('agentKeys.person_title')}>{t('agentKeys.person_body')}</Empty>
        </div>
      </section>
    );
  }

  const confirmRevoke = async () => {
    if (!revoking) return;
    setRevokeBusy(true);
    setRevokeError(null);
    try {
      await keysRevoke(client, revoking.id);
      toast(t('agentKeys.revoked_toast', { name: revoking.name }));
      setRevoking(null);
      state.reload();
    } catch (error) {
      setRevokeError(error);
    } finally {
      setRevokeBusy(false);
    }
  };

  return (
    <>
      <section class="panel" aria-labelledby="agent-keys-title">
        <div class="panel-head">
          <h2 id="agent-keys-title">{t('agentKeys.title')}</h2>
        </div>
        <p class="panel-body muted">{t('agentKeys.intro')}</p>
        {state.loading && !state.data && <Skeleton rows={3} label={t('agentKeys.loading')} />}
        {state.error && !state.data && <ErrorBanner error={state.error} onRetry={state.reload} />}
        {state.data &&
          (state.data.keys.length === 0 ? (
            <div class="panel-body">
              <Empty title={t('agentKeys.empty_title')}>{t('agentKeys.empty_body')}</Empty>
            </div>
          ) : (
            <div class="table-wrap">
              <table class="ledger cards">
                <thead>
                  <tr>
                    <th scope="col">{t('agentKeys.col_name')}</th>
                    <th scope="col">{t('agentKeys.col_key')}</th>
                    <th scope="col">{t('agentKeys.col_tier')}</th>
                    <th scope="col">{t('agentKeys.col_limits')}</th>
                    <th scope="col">{t('agentKeys.col_last_used')}</th>
                    <th scope="col">{t('agentKeys.col_status')}</th>
                    <th scope="col">
                      <span class="sr-only">{t('agentKeys.col_actions')}</span>
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {state.data.keys.map((key) => (
                    <tr key={key.id}>
                      <td class="main" data-label={t('agentKeys.col_name')}>
                        {key.name}
                        <div class="muted small">{t('agentKeys.created_at', { when: formatWhen(key.created_at) })}</div>
                      </td>
                      <td class="code" data-label={t('agentKeys.col_key')}>
                        {key.prefix ?? t('common.empty_cell')}
                      </td>
                      <td data-label={t('agentKeys.col_tier')}>{tierText(key.tier)}</td>
                      <td class="num limits" data-label={t('agentKeys.col_limits')}>
                        {limitsSummary(key).map((line) => (
                          <div key={line}>
                            <Ltr>{line}</Ltr>
                          </div>
                        ))}
                      </td>
                      <td data-label={t('agentKeys.col_last_used')}>
                        {key.last_used_at ? formatWhen(key.last_used_at) : <span class="muted">{t('agentKeys.never')}</span>}
                      </td>
                      <td data-label={t('agentKeys.col_status')}>
                        {key.revoked_at ? (
                          <span class="badge revoked">{t('agentKeys.revoked_at', { when: formatWhen(key.revoked_at) })}</span>
                        ) : (
                          <span class="badge active">{t('agentKeys.active')}</span>
                        )}
                      </td>
                      <td class="actions">
                        {!key.revoked_at && (
                          <button
                            type="button"
                            class="btn btn-sm btn-danger"
                            aria-label={t('agentKeys.revoke_named', { name: key.name })}
                            onClick={() => {
                              setRevokeError(null);
                              setRevoking(key);
                            }}
                          >
                            {t('agentKeys.revoke')}
                          </button>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          ))}
      </section>

      <section class="panel" aria-labelledby="usage-title">
        <div class="panel-head">
          <h2 id="usage-title">{t('agentKeys.use_title')}</h2>
        </div>
        <div class="panel-body stack">
          <p class="muted">{tRich('agentKeys.use_body', { token: <code>{TOKEN_PLACEHOLDER}</code> })}</p>
          <UsageSnippets baseUrl={client.baseUrl} token={TOKEN_PLACEHOLDER} />
        </div>
      </section>

      <AgentKeyCreate
        open={creating}
        onClose={() => onCreatingChange(false)}
        onCreated={(output) => {
          onCreatingChange(false);
          setCreated(output);
          state.reload();
        }}
      />
      <ShowOnceDialog output={created} baseUrl={client.baseUrl} onClose={() => setCreated(null)} />
      <ConfirmDialog
        open={revoking !== null}
        onClose={() => setRevoking(null)}
        title={t('agentKeys.revoke_title', { name: revoking?.name ?? t('agentKeys.this_key') })}
        confirmLabel={t('agentKeys.revoke_confirm')}
        danger
        busy={revokeBusy}
        error={revokeError}
        onConfirm={() => void confirmRevoke()}
      >
        <p>{tRich('agentKeys.revoke_body', { prefix: <span class="mono">{revoking?.prefix}</span> })}</p>
      </ConfirmDialog>
    </>
  );
}
