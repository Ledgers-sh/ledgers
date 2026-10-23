import { useState } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import {
  allocationRulesList,
  allocationRulesSet,
  connectionRuns,
  connectionsCreate,
  connectionsDelete,
  connectionsList,
  connectionsSyncNow,
  type AllocationRule,
  type AllocationListOutput,
  type ConnectionView,
  type ConnectorView,
  type RunView,
} from '../api/connect';
import { ApiError } from '../api/client';
import { useSession } from '../session';
import { t, type MessageKey } from '../i18n';
import { useAsync } from '../hooks';
import { describeError, ErrorBanner, Skeleton, ConfirmDialog, useToasts, Toasts } from './common';
import { Select } from './Select';
import { BRAND_MARKS } from './brandMarks';
import { Icon } from './icons';

/** The sources Ledgers connects, with where to make a least-privilege token. */
const SOURCE_HELP: Record<string, { imports: MessageKey; scopes: MessageKey; tokenUrl: string; tokenLabel: MessageKey }> = {
  cloudflare: {
    imports: 'integrations.cf_imports',
    scopes: 'integrations.scopes_cloudflare',
    tokenUrl: 'https://developers.cloudflare.com/fundamentals/api/get-started/create-token/',
    tokenLabel: 'integrations.create_token',
  },
  anthropic: {
    imports: 'integrations.ant_imports',
    scopes: 'integrations.scopes_anthropic',
    tokenUrl: 'https://platform.claude.com/docs/en/manage-claude/usage-cost-api',
    tokenLabel: 'integrations.create_token',
  },
  planetscale: {
    imports: 'integrations.ps_imports',
    scopes: 'integrations.scopes_planetscale',
    tokenUrl: 'https://planetscale.com/docs/api/reference/service-tokens',
    tokenLabel: 'integrations.create_token',
  },
};

/** Sources on the roadmap. Listed as text only: no button, no link, no form. */
const COMING_SOON = ['MiniMax', 'OpenAI', 'GitHub', 'Stripe', 'Polar', 'Google Workspace', 'AWS', 'Vercel', 'Resend'];

function Mark({ connector }: { connector: string }) {
  const mark = BRAND_MARKS[connector];
  if (!mark) return <span class="brand-mark brand-mark-empty" aria-hidden="true" />;
  return (
    <svg class="brand-mark" viewBox="0 0 24 24" role="img" aria-label={mark.title} focusable="false">
      <path d={mark.path} />
    </svg>
  );
}

function Notice({ tone, children }: { tone: 'ok' | 'error'; children: ComponentChildren }) {
  return (
    <p class={`banner ${tone === 'error' ? 'error' : ''}`} role={tone === 'error' ? 'alert' : 'status'}>
      {children}
    </p>
  );
}

/** Settings → Integrations: the gallery, the connections, their mapping and sync history. */
export function IntegrationsPanel() {
  const { client } = useSession();
  const { items: toasts, push, dismiss } = useToasts();
  const state = useAsync(
    async () => {
      const [list, allocation] = await Promise.all([connectionsList(client), allocationRulesList(client)]);
      return { list, allocation };
    },
    [client.baseUrl, client.apiKey],
  );
  const [picked, setPicked] = useState<string | null>(null);
  const [openId, setOpenId] = useState<string | null>(null);
  const person = !client.apiKey;

  if (state.loading && !state.data) return <Skeleton rows={4} label={t('integrations.loading')} />;
  if (state.error && !state.data) return <ErrorBanner error={state.error} onRetry={state.reload} />;
  if (!state.data) return null;
  const { list, allocation } = state.data;
  const connectors = list.connectors;
  const connections = list.connections;

  return (
    <div class="integrations">
      {!person && <p class="note">{t('integrations.agent_note')}</p>}

      <section class="integrations-section" aria-labelledby="integrations-gallery-title">
        <h3 id="integrations-gallery-title" class="section-title">{t('integrations.gallery_title')}</h3>
        <ul class="gallery" role="list">
          {connectors.map((connector) => (
            <li key={connector.id} class="gallery-card">
              <div class="gallery-head">
                <Mark connector={connector.id} />
                <div>
                  <h4 class="gallery-name">{connector.name}</h4>
                  <p class="muted gallery-imports">{t(SOURCE_HELP[connector.id]?.imports ?? 'integrations.cf_imports')}</p>
                </div>
              </div>
              <p class="muted gallery-hosts mono">{connector.hosts.join(', ')}</p>
              {person && (
                <button
                  type="button"
                  class="btn btn-primary"
                  aria-expanded={picked === connector.id}
                  onClick={() => setPicked(picked === connector.id ? null : connector.id)}
                >
                  {t('integrations.connect')}
                  <span class="sr-only"> {connector.name}</span>
                </button>
              )}
            </li>
          ))}
        </ul>
        {picked && person && (
          <ConnectForm
            key={picked}
            connector={connectors.find((c) => c.id === picked)!}
            units={allocation.units}
            onCancel={() => setPicked(null)}
            onConnected={(created) => {
              setPicked(null);
              push(t('integrations.saved', { name: created.name }));
              state.reload();
            }}
          />
        )}
      </section>

      <section class="integrations-section" aria-labelledby="integrations-connected-title">
        <h3 id="integrations-connected-title" class="section-title">{t('integrations.connected_title')}</h3>
        {connections.length === 0 ? (
          <p class="muted">{t('integrations.no_connections')}</p>
        ) : (
          <ul class="connection-list" role="list">
            {connections.map((connection) => (
              <li key={connection.id} class="connection-card">
                <div class="connection-head">
                  <Mark connector={connection.connector} />
                  <div class="grow">
                    <h4 class="connection-name">
                      {connection.name}
                      {connection.is_demo && <span class="badge demo-badge">{t('integrations.demo')}</span>}
                    </h4>
                    <p class="muted">
                      {connection.connector_name} ·{' '}
                      {connection.last_sync_at
                        ? t('integrations.last_sync', { when: connection.last_sync_at.slice(0, 16).replace('T', ' ') })
                        : t('integrations.never_synced')}
                    </p>
                  </div>
                  <button
                    type="button"
                    class="btn btn-sm"
                    aria-expanded={openId === connection.id}
                    onClick={() => setOpenId(openId === connection.id ? null : connection.id)}
                  >
                    {t('integrations.manage')}
                    <span class="sr-only"> {connection.name}</span>
                  </button>
                </div>
                {openId === connection.id && (
                  <ConnectionDetail
                    connection={connection}
                    allocation={allocation}
                    person={person}
                    onChanged={state.reload}
                    notify={push}
                    onDeleted={() => {
                      setOpenId(null);
                      state.reload();
                    }}
                  />
                )}
              </li>
            ))}
          </ul>
        )}
      </section>

      <section class="integrations-section coming-soon" aria-labelledby="integrations-soon-title">
        <h3 id="integrations-soon-title" class="section-title">{t('integrations.coming_soon_title')}</h3>
        <p class="muted">{t('integrations.coming_soon_body')}</p>
        <ul class="coming-soon-list" role="list">
          {COMING_SOON.map((name) => (
            <li key={name}>{name}</li>
          ))}
        </ul>
      </section>
      <Toasts items={toasts} dismiss={dismiss} />
    </div>
  );
}

/** The connect form: name, the connector's config, the expense account, the token (write-only). */
function ConnectForm({
  connector,
  units,
  onCancel,
  onConnected,
}: {
  connector: ConnectorView;
  units: AllocationListOutput['units'];
  onCancel: () => void;
  onConnected: (created: ConnectionView) => void;
}) {
  const { client } = useSession();
  const help = SOURCE_HELP[connector.id];
  const [name, setName] = useState(connector.name);
  const [account, setAccount] = useState('');
  const [organization, setOrganization] = useState('');
  const [cloudAccount, setCloudAccount] = useState('');
  const [currency, setCurrency] = useState('USD');
  const [vat, setVat] = useState('');
  const [defaultUnit, setDefaultUnit] = useState(units[0]?.code ?? '');
  const [token, setToken] = useState('');
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<unknown>(null);

  const needsCloudAccount = connector.config_keys.includes('account_id');
  const needsOrganization = connector.config_keys.includes('organization');
  const needsCurrency = connector.config_keys.includes('currency');

  const submit = async (event: Event) => {
    event.preventDefault();
    const next: Record<string, string> = {};
    if (name.trim() === '' || name.trim().length > 80) next.name = 'integrations.name_error';
    if (token.trim() === '') next.token = 'integrations.token_error';
    if (account.trim() === '') next.account = 'integrations.expense_hint';
    if (needsCloudAccount && cloudAccount.trim() === '') next.cloudAccount = 'integrations.account_id';
    if (needsOrganization && organization.trim() === '') next.organization = 'integrations.organization';
    if (vat.trim() !== '' && !/^\d{1,5}$/.test(vat.trim())) next.vat = 'integrations.vat_hint';
    setErrors(next);
    if (Object.keys(next).length > 0) return;
    const config: Record<string, unknown> = { expense_account: account.trim() };
    if (vat.trim() !== '') config.vat_rate_bp = Number(vat.trim());
    if (needsCloudAccount) config.account_id = cloudAccount.trim();
    if (needsOrganization) config.organization = organization.trim();
    if (needsCurrency) config.currency = currency.trim().toUpperCase();
    setBusy(true);
    setFailure(null);
    try {
      const created = await connectionsCreate(client, {
        connector: connector.id,
        name: name.trim(),
        config,
        default_unit: defaultUnit,
        token: token.trim(),
      });
      setToken('');
      onConnected(created);
    } catch (error) {
      setFailure(error);
    } finally {
      setBusy(false);
      setToken('');
    }
  };

  const unitOptions = units.map((unit) => ({ value: unit.code, label: `${unit.name} (${unit.code})` }));

  return (
    <form class="connect-form panel" onSubmit={submit} noValidate aria-labelledby="connect-form-title" autocomplete="off">
      <h4 id="connect-form-title" class="section-title">{t('integrations.form_title', { connector: connector.name })}</h4>
      <p class="hint">{t(help?.scopes ?? 'integrations.scopes_cloudflare')}</p>
      <p class="hint">
        <a href={help?.tokenUrl ?? '#'} target="_blank" rel="noopener noreferrer">
          {t(help?.tokenLabel ?? 'integrations.create_token')}
        </a>
      </p>
      <label class="field">
        <span class="field-label">{t('integrations.name')}</span>
        <input class="input" value={name} onInput={(e) => setName((e.target as HTMLInputElement).value)} aria-invalid={errors.name ? 'true' : undefined} aria-describedby="connect-name-hint" />
        <span id="connect-name-hint" class="hint">{t('integrations.name_hint')}</span>
        {errors.name && <span class="field-error">{t(errors.name as MessageKey)}</span>}
      </label>
      {needsCloudAccount && (
        <label class="field">
          <span class="field-label">{t('integrations.account_id')}</span>
          <input class="input mono" value={cloudAccount} onInput={(e) => setCloudAccount((e.target as HTMLInputElement).value)} aria-invalid={errors.cloudAccount ? 'true' : undefined} />
          {errors.cloudAccount && <span class="field-error">{t('integrations.account_id')}</span>}
        </label>
      )}
      {needsOrganization && (
        <label class="field">
          <span class="field-label">{t('integrations.organization')}</span>
          <input class="input mono" value={organization} onInput={(e) => setOrganization((e.target as HTMLInputElement).value)} aria-invalid={errors.organization ? 'true' : undefined} />
          {errors.organization && <span class="field-error">{t('integrations.organization')}</span>}
        </label>
      )}
      {needsCurrency && (
        <label class="field">
          <span class="field-label">{t('integrations.currency')}</span>
          <input class="input mono" maxLength={3} value={currency} onInput={(e) => setCurrency((e.target as HTMLInputElement).value)} aria-describedby="connect-currency-hint" />
          <span id="connect-currency-hint" class="hint">{t('integrations.currency_hint')}</span>
        </label>
      )}
      <label class="field">
        <span class="field-label">{t('integrations.expense_account')}</span>
        <input class="input mono" value={account} onInput={(e) => setAccount((e.target as HTMLInputElement).value)} aria-describedby="connect-account-hint" aria-invalid={errors.account ? 'true' : undefined} />
        <span id="connect-account-hint" class="hint">{t('integrations.expense_hint')}</span>
      </label>
      <label class="field">
        <span class="field-label">{t('integrations.vat_rate')}</span>
        <input class="input mono" inputMode="numeric" value={vat} onInput={(e) => setVat((e.target as HTMLInputElement).value)} aria-describedby="connect-vat-hint" aria-invalid={errors.vat ? 'true' : undefined} />
        <span id="connect-vat-hint" class="hint">{t('integrations.vat_hint')}</span>
      </label>
      <div class="field">
        <span class="field-label" id="connect-unit-label">{t('integrations.default_unit')}</span>
        <Select id="connect-unit" labelledBy="connect-unit-label" value={defaultUnit} options={unitOptions} onChange={setDefaultUnit} />
        <span class="hint">{t('integrations.default_unit_hint')}</span>
      </div>
      <label class="field">
        <span class="field-label">{t('integrations.token')}</span>
        <input
          class="input mono"
          type="password"
          autocomplete="new-password"
          spellcheck={false}
          value={token}
          onInput={(e) => setToken((e.target as HTMLInputElement).value)}
          aria-describedby="connect-token-hint"
          aria-invalid={errors.token ? 'true' : undefined}
        />
        <span id="connect-token-hint" class="hint">{t('integrations.token_hint')}</span>
        {errors.token && <span class="field-error">{t(errors.token as MessageKey)}</span>}
      </label>
      <p class="hint">{t('integrations.key_warning')}</p>
      {failure !== null && <Notice tone="error">{describeError(failure).title}{failure instanceof ApiError && failure.detail ? `: ${failure.detail}` : ''}</Notice>}
      <div class="form-actions">
        <button type="button" class="btn btn-ghost" onClick={onCancel}>
          {t('integrations.cancel')}
        </button>
        <button type="submit" class="btn btn-primary" disabled={busy}>
          {t('integrations.save')}
        </button>
      </div>
    </form>
  );
}

/** One connection's detail: sync now, the sync history, the mapping and the needs-mapping queue. */
function ConnectionDetail({
  connection,
  allocation,
  person,
  onChanged,
  onDeleted,
  notify,
}: {
  connection: ConnectionView;
  allocation: AllocationListOutput;
  person: boolean;
  onChanged: () => void;
  onDeleted: () => void;
  notify: (message: string, kind?: 'ok' | 'error') => void;
}) {
  const { client } = useSession();
  const runs = useAsync(() => connectionRuns(client, connection.id, 10), [client.baseUrl, client.apiKey, connection.id, connection.last_sync_at]);
  const [syncing, setSyncing] = useState(false);
  const [syncError, setSyncError] = useState<unknown>(null);
  const [confirming, setConfirming] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [deleteError, setDeleteError] = useState<unknown>(null);

  const sync = async () => {
    setSyncing(true);
    setSyncError(null);
    try {
      const run: RunView = await connectionsSyncNow(client, connection.id);
      if (run.status === 'failed') {
        setSyncError(new Error(run.error_message ?? ''));
        notify(t('integrations.sync_failed', { reason: run.error_message ?? '' }), 'error');
      } else {
        notify(t('integrations.synced', { invoices: String(run.invoices_found), bills: String(run.bills_created) }));
      }
      runs.reload();
      onChanged();
    } catch (error) {
      setSyncError(error);
    } finally {
      setSyncing(false);
    }
  };

  const remove = async () => {
    setDeleting(true);
    setDeleteError(null);
    try {
      await connectionsDelete(client, connection.id);
      notify(t('integrations.disconnected', { name: connection.name }));
      setConfirming(false);
      onDeleted();
    } catch (error) {
      setDeleteError(error);
    } finally {
      setDeleting(false);
    }
  };

  return (
    <div class="connection-detail">
      {connection.is_demo && <p class="note">{t('integrations.demo_note')}</p>}
      {person && (
        <div class="row-actions">
          <button type="button" class="btn btn-primary" onClick={sync} disabled={syncing}>
            {syncing ? t('integrations.syncing') : t('integrations.sync_now')}
          </button>
          {!connection.is_demo && (
            <button type="button" class="btn btn-ghost" onClick={() => setConfirming(true)}>
              {t('integrations.disconnect')}
            </button>
          )}
        </div>
      )}
      {syncError !== null && <Notice tone="error">{describeError(syncError).title}</Notice>}
      <h5 class="section-title">{t('integrations.history')}</h5>
      {runs.loading && !runs.data ? (
        <Skeleton rows={2} label={t('integrations.loading')} />
      ) : runs.error ? (
        <ErrorBanner error={runs.error} onRetry={runs.reload} />
      ) : runs.data && runs.data.runs.length > 0 ? (
        <table class="ledger cards integrations-runs">
          <caption class="sr-only">{t('integrations.history')}</caption>
          <thead>
            <tr>
              <th scope="col">{t('integrations.window_label')}</th>
              <th scope="col">{t('integrations.status')}</th>
              <th scope="col">{t('integrations.result')}</th>
            </tr>
          </thead>
          <tbody>
            {runs.data.runs.map((run) => (
              <tr key={run.id}>
                <td data-label={t('integrations.window_label')} class="mono">
                  {t('integrations.window', { since: run.window_since, until: run.window_until })}
                </td>
                <td data-label={t('integrations.status')}>
                  <span class={`badge ${run.status === 'failed' ? 'warn' : 'ok'}`}>
                    {run.status === 'failed' ? t('integrations.run_status_failed') : t('integrations.run_status_ok')}
                  </span>
                </td>
                <td data-label={t('integrations.result')}>
                  {run.status === 'failed' ? (
                    <span class="muted">{run.error_message}</span>
                  ) : (
                    t('integrations.run_summary', {
                      invoices: String(run.invoices_found),
                      bills: String(run.bills_created),
                      usage: String(run.usage_rows),
                    })
                  )}
                  {run.notes.length > 0 && (
                    <details class="run-notes">
                      <summary>{t('integrations.notes')}</summary>
                      <ul>
                        {run.notes.map((note) => (
                          <li key={note}>{note}</li>
                        ))}
                      </ul>
                    </details>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : (
        <p class="muted">{t('integrations.history_empty')}</p>
      )}
      <MappingEditor connection={connection} allocation={allocation} person={person} onSaved={onChanged} notify={notify} />
      <ConfirmDialog
        open={confirming}
        onClose={() => setConfirming(false)}
        title={t('integrations.disconnect_title', { name: connection.name })}
        confirmLabel={t('integrations.disconnect')}
        onConfirm={remove}
        busy={deleting}
        error={deleteError}
        danger
      >
        <p>{t('integrations.disconnect_body')}</p>
      </ConfirmDialog>
    </div>
  );
}

/** The mapping: resource id patterns to units, the default unit, and the queue of resources that need one. */
function MappingEditor({
  connection,
  allocation,
  person,
  onSaved,
  notify,
}: {
  connection: ConnectionView;
  allocation: AllocationListOutput;
  person: boolean;
  onSaved: () => void;
  notify: (message: string, kind?: 'ok' | 'error') => void;
}) {
  const { client } = useSession();
  const mine = allocation.connections.find((row) => row.connection_id === connection.id);
  const [rules, setRules] = useState<AllocationRule[]>(mine?.rules ?? []);
  const [defaultUnit, setDefaultUnit] = useState(mine?.default_unit ?? connection.default_unit);
  const [saving, setSaving] = useState(false);
  const [failure, setFailure] = useState<unknown>(null);
  const unitOptions = allocation.units.map((unit) => ({ value: unit.code, label: `${unit.name} (${unit.code})` }));
  const unmapped = mine?.unmapped ?? [];

  const save = async (next: AllocationRule[], unit: string) => {
    setSaving(true);
    setFailure(null);
    try {
      const result = await allocationRulesSet(client, connection.id, next, unit);
      setRules(result.rules);
      notify(t('integrations.mapping_saved', { resplit: String(result.resplit), kept: String(result.kept_frozen) }));
      onSaved();
    } catch (error) {
      setFailure(error);
    } finally {
      setSaving(false);
    }
  };

  const mapResource = (resourceId: string, unit: string) => {
    const next = [...rules.filter((rule) => rule.pattern !== resourceId), { pattern: resourceId, unit_code: unit }];
    setRules(next);
    void save(next, defaultUnit);
  };

  return (
    <div class="mapping">
      <h5 class="section-title">{t('integrations.mapping_title')}</h5>
      <p class="muted">{t('integrations.mapping_intro')}</p>
      <h6 class="section-subtitle">{t('integrations.queue_title')}</h6>
      {unmapped.length === 0 ? (
        <p class="muted">{t('integrations.queue_empty')}</p>
      ) : (
        <ul class="queue" role="list">
          {unmapped.map((resource) => (
            <li key={`${resource.resource_kind}:${resource.resource_id}`} class="queue-item">
              <span class="mono">{resource.resource_name}</span>
              <span class="muted">{t('integrations.queue_periods', { periods: resource.periods.join(', ') })}</span>
              {resource.suggested_unit && <span class="muted">{t('integrations.suggested', { unit: resource.suggested_unit })}</span>}
              {person && (
                <button
                  type="button"
                  class="btn btn-sm"
                  disabled={saving || !(resource.suggested_unit ?? defaultUnit)}
                  onClick={() => mapResource(resource.resource_id, resource.suggested_unit ?? defaultUnit)}
                >
                  {t('integrations.map_to', { unit: resource.suggested_unit ?? defaultUnit })}
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
      <div class="field">
        <span class="field-label" id={`default-unit-${connection.id}`}>{t('integrations.default_unit')}</span>
        {person ? (
          <Select id={`default-${connection.id}`} labelledBy={`default-unit-${connection.id}`} value={defaultUnit} options={unitOptions} onChange={setDefaultUnit} />
        ) : (
          <span class="mono">{defaultUnit}</span>
        )}
      </div>
      <table class="ledger cards mapping-table">
        <caption class="sr-only">{t('integrations.mapping_title')}</caption>
        <thead>
          <tr>
            <th scope="col">{t('integrations.rule_pattern')}</th>
            <th scope="col">{t('integrations.rule_unit')}</th>
            {person && <th scope="col"><span class="sr-only">{t('integrations.remove_rule')}</span></th>}
          </tr>
        </thead>
        <tbody>
          {rules.map((rule, index) => (
            <tr key={`${rule.pattern}-${index}`}>
              <td data-label={t('integrations.rule_pattern')} class="mono">{rule.pattern}</td>
              <td data-label={t('integrations.rule_unit')} class="mono">{rule.unit_code}</td>
              {person && (
                <td class="actions">
                  <button type="button" class="btn btn-sm btn-ghost" onClick={() => setRules(rules.filter((_, i) => i !== index))}>
                    <Icon name="close" />
                    <span class="sr-only"> {rule.pattern}</span>
                  </button>
                </td>
              )}
            </tr>
          ))}
        </tbody>
      </table>
      {person && (
        <div class="row-actions">
          <button type="button" class="btn" onClick={() => setRules([...rules, { pattern: '', unit_code: defaultUnit }])}>
            {t('integrations.add_rule')}
          </button>
          <button type="button" class="btn btn-primary" disabled={saving} onClick={() => save(rules.filter((rule) => rule.pattern.trim() !== ''), defaultUnit)}>
            {t('integrations.save_mapping')}
          </button>
        </div>
      )}
      {failure !== null && <Notice tone="error">{describeError(failure).title}</Notice>}
    </div>
  );
}
