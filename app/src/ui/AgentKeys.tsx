import { useState } from 'preact/hooks';
import { keysList, keysRevoke } from '../api/commands';
import type { AgentKeyView, KeysCreateOutput } from '../api/types';
import { useAsync } from '../hooks';
import { TOKEN_PLACEHOLDER, tierLabelKey } from '../lib/agentKeys';
import { useSession } from '../session';
import { intlLocale, t, tRich, type MessageKey } from '../i18n';
import { Ltr } from '../i18n/Ltr';
import { ConfirmDialog, Empty, ErrorBanner, Skeleton } from './common';
import { Icon } from './icons';
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
        <h2 id="agent-keys-title">{t('agentKeys.title')}</h2>
        <Empty title={t('agentKeys.person_title')}>{t('agentKeys.person_body')}</Empty>
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
    <section class="settings-section" aria-labelledby="agent-keys-title">
      <div class="section-head">
        <div>
          <h2 id="agent-keys-title">{t('agentKeys.title')}</h2>
          <p class="muted">{t('agentKeys.intro')}</p>
        </div>
        <button type="button" class="btn btn-primary" onClick={() => setCreating(true)}>
          <Icon name="plus" />
          {t('agentKeys.create')}
        </button>
      </div>

      {state.loading && !state.data && <Skeleton rows={3} label={t('agentKeys.loading')} />}
      {state.error && !state.data && <ErrorBanner error={state.error} onRetry={state.reload} />}
      {state.data &&
        (state.data.keys.length === 0 ? (
          <Empty title={t('agentKeys.empty_title')}>{t('agentKeys.empty_body')}</Empty>
        ) : (
          <div class="panel">
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
                      <td class="mono" data-label={t('agentKeys.col_key')}>
                        {key.prefix ?? t('common.empty_cell')}
                      </td>
                      <td data-label={t('agentKeys.col_tier')}>{tierText(key.tier)}</td>
                      <td class="num" data-label={t('agentKeys.col_limits')}>
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
                          <span class="status revoked">{t('agentKeys.revoked_at', { when: formatWhen(key.revoked_at) })}</span>
                        ) : (
                          <span class="status active">{t('agentKeys.active')}</span>
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
          </div>
        ))}

      <div class="panel usage">
        <div class="panel-body">
          <h3>{t('agentKeys.use_title')}</h3>
          <p class="muted">
            {tRich('agentKeys.use_body', { token: <code>{TOKEN_PLACEHOLDER}</code> })}
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
        title={t('agentKeys.revoke_title', { name: revoking?.name ?? t('agentKeys.this_key') })}
        confirmLabel={t('agentKeys.revoke_confirm')}
        danger
        busy={revokeBusy}
        error={revokeError}
        onConfirm={() => void confirmRevoke()}
      >
        <p>{tRich('agentKeys.revoke_body', { prefix: <span class="mono">{revoking?.prefix}</span> })}</p>
      </ConfirmDialog>
    </section>
  );
}
