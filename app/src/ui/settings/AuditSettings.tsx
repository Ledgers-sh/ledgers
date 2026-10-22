import { useEffect, useState } from 'preact/hooks';
import { auditList, type AuditEventView, type AuditListParams } from '../../api/settings';
import { t, type MessageKey } from '../../i18n';
import { useSession } from '../../session';
import { auditCsv } from '../../lib/audit';
import { describeError, Empty, Marker } from '../common';
import { Select } from '../Select';
import { formatWhen } from '../AgentKeys';
import { FormProblem, LoadingLine, SettingsPanel, StateBadge } from './common';

/** One page of the audit read. */
export const AUDIT_PAGE = 50;
/** The most events one CSV export holds; a larger range asks for narrower dates. */
export const AUDIT_EXPORT_LIMIT = 2000;

interface Filters {
  actor_kind: '' | 'person' | 'agent';
  command: string;
  outcome: '' | 'allowed' | 'needs-approval' | 'denied' | 'error';
  from: string;
  to: string;
}

const NO_FILTERS: Filters = { actor_kind: '', command: '', outcome: '', from: '', to: '' };

/** The query the read takes for the filters: empty fields are left out. */
export function paramsOf(filters: Filters, cursor?: number): AuditListParams {
  return {
    limit: AUDIT_PAGE,
    before_seq: cursor,
    actor_kind: filters.actor_kind || undefined,
    command: filters.command.trim() || undefined,
    outcome: filters.outcome || undefined,
    from: filters.from || undefined,
    to: filters.to || undefined,
  };
}

const OUTCOME_LABEL: Record<AuditEventView['outcome'], MessageKey> = {
  allowed: 'settings.outcome_allowed',
  'needs-approval': 'settings.outcome_needs_approval',
  denied: 'settings.outcome_denied',
  error: 'settings.outcome_error',
};

const OUTCOME_TONE: Record<AuditEventView['outcome'], 'open' | 'closed' | 'soft' | 'neutral'> = {
  allowed: 'neutral',
  'needs-approval': 'soft',
  denied: 'closed',
  error: 'closed',
};

/** Settings → Audit log: who did what and when, searchable, paged, and exportable as CSV. */
export function AuditSettings() {
  const { client, toast } = useSession();
  const [draft, setDraft] = useState<Filters>(NO_FILTERS);
  const [filters, setFilters] = useState<Filters>(NO_FILTERS);
  const [events, setEvents] = useState<AuditEventView[] | null>(null);
  const [cursor, setCursor] = useState<number | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const [exporting, setExporting] = useState(false);

  useEffect(() => {
    let live = true;
    setEvents(null);
    setError(null);
    setLoading(true);
    auditList(client, paramsOf(filters)).then(
      (page) => {
        if (!live) return;
        setEvents(page.events);
        setCursor(page.next_before_seq);
        setLoading(false);
      },
      (err: unknown) => {
        if (!live) return;
        setError(err);
        setLoading(false);
      },
    );
    return () => {
      live = false;
    };
  }, [client.baseUrl, client.apiKey, filters]);

  async function loadMore() {
    if (cursor === null) return;
    setLoading(true);
    setError(null);
    try {
      const page = await auditList(client, paramsOf(filters, cursor));
      setEvents([...(events ?? []), ...page.events]);
      setCursor(page.next_before_seq);
    } catch (err) {
      setError(err);
    } finally {
      setLoading(false);
    }
  }

  async function exportCsv() {
    setExporting(true);
    setError(null);
    try {
      const all: AuditEventView[] = [];
      let next: number | undefined;
      let more = true;
      while (more) {
        const page = await auditList(client, { ...paramsOf(filters, next), limit: 200 });
        all.push(...page.events);
        if (all.length > AUDIT_EXPORT_LIMIT) throw new Error(t('settings.export_too_large', { max: AUDIT_EXPORT_LIMIT }));
        next = page.next_before_seq ?? undefined;
        more = next !== undefined;
      }
      const blob = new Blob([auditCsv(all)], { type: 'text/csv;charset=utf-8' });
      const url = URL.createObjectURL(blob);
      const link = document.createElement('a');
      link.href = url;
      link.download = `ledgers-audit-${new Date().toISOString().slice(0, 10)}.csv`;
      document.body.append(link);
      link.click();
      link.remove();
      setTimeout(() => URL.revokeObjectURL(url), 0);
      toast(t('settings.export_done', { count: all.length }));
    } catch (err) {
      setError(err);
    } finally {
      setExporting(false);
    }
  }

  const set = (field: keyof Filters) => (event: Event) => {
    setDraft({ ...draft, [field]: (event.target as HTMLInputElement | HTMLSelectElement).value });
  };

  return (
    <SettingsPanel id="audit-title" title={t('settings.audit_title')} actions={
      <button type="button" class="btn" disabled={exporting || events === null} onClick={() => void exportCsv()}>
        {exporting ? t('settings.exporting') : t('settings.export_csv')}
      </button>
    }>
      <form
        class="audit-filters"
        aria-label={t('settings.audit_filters')}
        onSubmit={(event) => {
          event.preventDefault();
          setFilters({ ...draft, command: draft.command.trim() });
        }}
      >
        <div class="field">
          <span id="audit-actor-label">{t('settings.filter_actor')}</span>
          <Select
            id="audit-actor"
            name="actor_kind"
            labelledBy="audit-actor-label"
            value={draft.actor_kind}
            options={[
              { value: '', label: t('settings.any') },
              { value: 'person', label: t('settings.actor_person') },
              { value: 'agent', label: t('settings.actor_agent') },
            ]}
            onChange={(value) => setDraft({ ...draft, actor_kind: value as Filters['actor_kind'] })}
          />
        </div>
        <label class="field">
          <span>{t('settings.filter_command')}</span>
          <input
            class="input mono"
            name="command"
            aria-label={t('settings.filter_command')}
            value={draft.command}
            placeholder="entries.post"
            maxLength={80}
            autoComplete="off"
            onInput={set('command')}
          />
        </label>
        <div class="field">
          <span id="audit-outcome-label">{t('settings.filter_outcome')}</span>
          <Select
            id="audit-outcome"
            name="outcome"
            labelledBy="audit-outcome-label"
            value={draft.outcome}
            options={[
              { value: '', label: t('settings.any') },
              { value: 'allowed', label: t('settings.outcome_allowed') },
              { value: 'needs-approval', label: t('settings.outcome_needs_approval') },
              { value: 'denied', label: t('settings.outcome_denied') },
              { value: 'error', label: t('settings.outcome_error') },
            ]}
            onChange={(value) => setDraft({ ...draft, outcome: value as Filters['outcome'] })}
          />
        </div>
        <label class="field">
          <span>{t('settings.filter_from')}</span>
          <input class="input mono" type="date" name="from" aria-label={t('settings.filter_from')} value={draft.from} onInput={set('from')} />
        </label>
        <label class="field">
          <span>{t('settings.filter_to')}</span>
          <input class="input mono" type="date" name="to" aria-label={t('settings.filter_to')} value={draft.to} onInput={set('to')} />
        </label>
        <div class="form-actions">
          <button type="button" class="btn btn-ghost" onClick={() => {
            setDraft(NO_FILTERS);
            setFilters(NO_FILTERS);
          }}>
            {t('settings.clear_filters')}
          </button>
          <button type="submit" class="btn btn-primary">
            {t('settings.apply_filters')}
          </button>
        </div>
      </form>

      <FormProblem error={error} />
      {loading && events === null ? <LoadingLine /> : null}
      {events !== null && events.length === 0 ? (
        <Empty title={t('settings.audit_empty_title')}>{t('settings.audit_empty_body')}</Empty>
      ) : null}
      {events !== null && events.length > 0 ? (
        <div class="table-wrap">
          <table class="settings-table audit-table">
            <thead>
              <tr>
                <th scope="col">{t('settings.col_when')}</th>
                <th scope="col">{t('settings.col_who')}</th>
                <th scope="col">{t('settings.col_what')}</th>
                <th scope="col">{t('settings.col_outcome')}</th>
                <th scope="col">{t('settings.col_objects')}</th>
              </tr>
            </thead>
            <tbody>
              {events.map((event) => (
                <tr key={event.seq}>
                  <td class="small">
                    <time dateTime={event.at}>{formatWhen(event.at)}</time>
                  </td>
                  <td>
                    <Marker by={event.actor_kind} />{' '}
                    <span class="mono small">{event.actor_id}</span>
                  </td>
                  <td>
                    <span class="mono">{event.command}</span>
                    {event.reason ? <span class="hint block">{event.reason}</span> : null}
                  </td>
                  <td>
                    <StateBadge tone={OUTCOME_TONE[event.outcome]}>{t(OUTCOME_LABEL[event.outcome])}</StateBadge>
                  </td>
                  <td class="mono small">{event.objects.join(', ')}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : null}
      {cursor !== null ? (
        <div class="row-actions">
          <button type="button" class="btn" disabled={loading} onClick={() => void loadMore()}>
            {t('settings.load_more')}
          </button>
        </div>
      ) : null}
      {error && events === null ? <p class="muted">{describeError(error).title}</p> : null}
    </SettingsPanel>
  );
}
