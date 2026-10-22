import { useState } from 'preact/hooks';
import { approvalDecide, approvalGet, approvalsBulkApprove, approvalsList, type ApprovalStatus, type ApprovalView } from '../api/approvals';
import { useAsync } from '../hooks';
import { useSession } from '../session';
import {
  formatTimestamp,
  shortHash,
  relativeTime,
  expiry,
  amountText,
  bulkPlan,
  bulkTotalMinor,
  classifyDecisionError,
  displayAmount,
  isBulkEligible,
  reauthUrl,
  resultEntry,
  type PlanMessage,
} from '../lib/approvals';
import { intlLocale, t, tn, tRich, type MessageKey } from '../i18n';
import { Ltr } from '../i18n/Ltr';
import { ConfirmDialog, Empty, ErrorBanner, PageHead, Skeleton, describeError } from './common';

const STATUS_KEY: Record<ApprovalStatus, MessageKey> = {
  pending: 'approvals.status_pending',
  approved: 'approvals.status_approved',
  rejected: 'approvals.status_rejected',
  expired: 'approvals.status_expired',
  superseded: 'approvals.status_superseded',
};

/** The agent that asked, in agent blue. */
export function AgentKey({ id }: { id: string }) {
  return (
    <span class="agent-key" title={t('approvals.requesting_key')}>
      <Ltr>{id}</Ltr>
    </span>
  );
}

export function RiskChip({ risk }: { risk: ApprovalView['risk'] }) {
  return <span class={`chip risk-${risk}`}>{t(risk === 'high' ? 'approvals.risk_high' : 'approvals.risk_low')}</span>;
}

export function StatusChip({ status }: { status: ApprovalStatus }) {
  return <span class={`chip status-${status}`}>{t(STATUS_KEY[status])}</span>;
}

/** When an approval expires, as words: "expires in 2 hours" while open, "expired 30 minutes ago" after. */
function expiryPhrase(expiresAt: string, now: number): string {
  const found = expiry(expiresAt, now, intlLocale());
  if (!found) return '';
  return t(found.open ? 'approvals.expires' : 'approvals.expired', { when: found.when });
}

/** Points at the journal entry a decision posted, or shows the command output as sent. */
function ResultReference({ result }: { result: unknown }) {
  if (result === null || result === undefined) return <span class="muted">{t('approvals.none')}</span>;
  const entry = resultEntry(result);
  if (entry) {
    return (
      <a class="row-link" href={`#/journal/${encodeURIComponent(entry.journalKey)}`}>
        <Ltr>{entry.entryId}</Ltr>
      </a>
    );
  }
  return <pre class="result-json">{JSON.stringify(result, null, 2)}</pre>;
}

function AgentKeyNotice() {
  return (
    <Empty title={t('approvals.agent_key_title')}>{t('approvals.agent_key_body')}</Empty>
  );
}

function labelOf(item: ApprovalView): string {
  return item.counterparty ?? item.command;
}

/** A selection's refusal, in the reader's language. */
function planMessageText(message: PlanMessage): string {
  if (message.kind === 'high-risk') return t('approvals.bulk_high_risk');
  const list = new Intl.ListFormat(intlLocale(), { style: 'long', type: 'conjunction' }).format(message.currencies);
  return t('approvals.bulk_mixed', { currencies: list });
}

/** A failed decision's message, in the reader's language. */
function staleText(reason: 'not-pending' | 'expired'): string {
  return t(reason === 'expired' ? 'approvals.expired_message' : 'approvals.not_pending');
}

type Tab = 'pending' | 'history';

export function Approvals() {
  const { client } = useSession();
  if (client.apiKey) return <AgentKeyNotice />;
  return <ApprovalsInbox />;
}

function ApprovalsInbox() {
  const { client, refreshApprovals } = useSession();
  const [tab, setTab] = useState<Tab>('pending');
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const [confirming, setConfirming] = useState(false);
  const [bulkBusy, setBulkBusy] = useState(false);
  const [notice, setNotice] = useState<{ kind: 'ok' | 'error'; message: string } | null>(null);
  const status: 'pending' | 'decided' = tab === 'pending' ? 'pending' : 'decided';
  // The list is tagged with the tab it was loaded for, so a tab switch shows loading, not the other tab's rows.
  const state = useAsync(
    async () => ({ status, list: await approvalsList(client, status, 50) }),
    [client.baseUrl, client.apiKey, status],
  );
  const loaded = state.data?.status === status ? state.data.list : undefined;
  const items = loaded?.items ?? [];
  const now = Date.now();
  const locale = intlLocale();
  // Only rows still on screen and still eligible can be selected: a reload drops the rest.
  const chosen = new Set([...selected].filter((id) => items.some((item) => item.id === id && isBulkEligible(item))));
  const plan = bulkPlan(items, chosen);
  const total = plan.currency ? bulkTotalMinor(items, chosen, plan.currency) : null;
  const totalText = plan.currency && total !== null ? displayAmount(total, plan.currency, locale) : null;
  const planText = plan.message ? planMessageText(plan.message) : null;

  function toggle(id: string, checked: boolean) {
    setSelected((prev) => {
      const next = new Set(prev);
      if (checked) next.add(id);
      else next.delete(id);
      return next;
    });
  }

  async function approveChosen() {
    setConfirming(false);
    setBulkBusy(true);
    setNotice(null);
    try {
      const output = await approvalsBulkApprove(client, [...chosen]);
      const count = output.results.length;
      setSelected(new Set());
      setNotice({ kind: 'ok', message: tn('approvals.approved_count', count) });
    } catch (error) {
      const failure = classifyDecisionError(error);
      let message = describeError(error).title;
      if (failure.kind === 'refused') message = failure.detail ?? t('approvals.bulk_refused');
      if (failure.kind === 'stale') message = staleText(failure.reason);
      setNotice({ kind: 'error', message });
    } finally {
      setBulkBusy(false);
      state.reload();
      refreshApprovals();
    }
  }

  const confirmTitle = tn('approvals.confirm_title', plan.count);
  const confirmAction = tn('approvals.confirm_action', plan.count);

  return (
    <>
      <PageHead title={t('approvals.title')}>{t('approvals.intro')}</PageHead>

      <div class="tabs" role="tablist" aria-label={t('approvals.tabs_label')}>
        {(['pending', 'history'] as const).map((name) => (
          <button
            key={name}
            type="button"
            role="tab"
            id={`tab-${name}`}
            class="tab"
            aria-selected={tab === name ? 'true' : 'false'}
            aria-controls="approvals-panel"
            onClick={() => {
              setTab(name);
              setNotice(null);
            }}
          >
            {name === 'pending' ? t('approvals.tab_pending') : t('approvals.tab_history')}
          </button>
        ))}
      </div>

      <section id="approvals-panel" class="panel" role="tabpanel" aria-labelledby={`tab-${tab}`}>
        {notice && (
          <div class={`banner ${notice.kind === 'error' ? 'error' : 'human'} notice`} role={notice.kind === 'error' ? 'alert' : 'status'}>
            <span class="grow">{notice.message}</span>
          </div>
        )}

        {state.loading && !loaded ? (
          <div class="panel-body">
            <Skeleton rows={4} label={t('approvals.loading')} />
          </div>
        ) : state.error && !loaded ? (
          <div class="panel-body">
            <ErrorBanner error={state.error} onRetry={state.reload} />
          </div>
        ) : tab === 'pending' ? (
          items.length === 0 ? (
            <div class="panel-body">
              <Empty title={t('approvals.empty_title')}>{t('approvals.empty_body')}</Empty>
            </div>
          ) : (
            <>
              {chosen.size > 0 && (
                <div class="bulk-bar" role="region" aria-label={t('approvals.bulk_region')}>
                  <span>
                    {tRich('approvals.selected', { count: <strong><Ltr>{String(plan.count)}</Ltr></strong> })}
                    {totalText && (
                      <>
                        {' · '}
                        <Ltr>{totalText}</Ltr>
                      </>
                    )}
                  </span>
                  {planText && (
                    <span class="bulk-message" role="status">
                      {planText}
                    </span>
                  )}
                  <span class="grow" />
                  <button type="button" class="btn btn-human" disabled={!plan.canApprove || bulkBusy} onClick={() => setConfirming(true)}>
                    {t('approvals.approve_selected', { count: plan.count })}
                  </button>
                  <button type="button" class="btn btn-ghost btn-sm" onClick={() => setSelected(new Set())}>
                    {t('approvals.clear')}
                  </button>
                </div>
              )}
              <div class="table-wrap">
                <table class="ledger cards">
                  <thead>
                    <tr>
                      <th scope="col">
                        <span class="sr-only">{t('approvals.select_all_label')}</span>
                      </th>
                      <th scope="col">{t('approvals.col_counterparty')}</th>
                      <th scope="col" class="num">
                        {t('term.amount')}
                      </th>
                      <th scope="col">{t('approvals.col_reason')}</th>
                      <th scope="col">{t('approvals.col_requested_by')}</th>
                      <th scope="col">{t('approvals.col_risk')}</th>
                      <th scope="col">{t('approvals.col_expires')}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {items.map((item) => {
                      const eligible = isBulkEligible(item);
                      return (
                        <tr key={item.id}>
                          <td class="check-cell" data-label={t('approvals.col_select')}>
                            <input
                              type="checkbox"
                              aria-label={t('approvals.select_row', { label: labelOf(item) })}
                              checked={chosen.has(item.id)}
                              disabled={!eligible || bulkBusy}
                              title={eligible ? undefined : t('approvals.bulk_high_risk')}
                              onChange={(event) => toggle(item.id, (event.target as HTMLInputElement).checked)}
                            />
                          </td>
                          <td class="lead" data-label={t('approvals.col_counterparty')}>
                            <a class="row-link" href={`#/approvals/${encodeURIComponent(item.id)}`}>
                              {labelOf(item)}
                            </a>
                          </td>
                          <td class="num" data-label={t('term.amount')}>
                            <Ltr>{amountText(item.amount, locale)}</Ltr>
                          </td>
                          <td data-label={t('approvals.col_reason')}>{item.reason}</td>
                          <td data-label={t('approvals.col_requested_by')}>
                            <AgentKey id={item.requested_by} />
                          </td>
                          <td data-label={t('approvals.col_risk')}>
                            <RiskChip risk={item.risk} />
                          </td>
                          <td data-label={t('approvals.col_expires')}>{expiryPhrase(item.expires_at, now)}</td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>
            </>
          )
        ) : items.length === 0 ? (
          <div class="panel-body">
            <Empty title={t('approvals.history_empty_title')}>{t('approvals.history_empty_body')}</Empty>
          </div>
        ) : (
          <div class="table-wrap">
            <table class="ledger cards">
              <thead>
                <tr>
                  <th scope="col">{t('approvals.col_status')}</th>
                  <th scope="col">{t('approvals.col_counterparty')}</th>
                  <th scope="col" class="num">
                    {t('term.amount')}
                  </th>
                  <th scope="col">{t('approvals.col_decided')}</th>
                  <th scope="col">{t('approvals.col_decided_by')}</th>
                  <th scope="col">{t('approvals.col_audit_ref')}</th>
                  <th scope="col">{t('approvals.col_result')}</th>
                </tr>
              </thead>
              <tbody>
                {items.map((item) => (
                  <tr key={item.id}>
                    <td data-label={t('approvals.col_status')}>
                      <StatusChip status={item.status} />
                    </td>
                    <td class="lead" data-label={t('approvals.col_counterparty')}>
                      <a class="row-link" href={`#/approvals/${encodeURIComponent(item.id)}`}>
                        {labelOf(item)}
                      </a>
                    </td>
                    <td class="num" data-label={t('term.amount')}>
                      <Ltr>{amountText(item.amount, locale)}</Ltr>
                    </td>
                    <td data-label={t('approvals.col_decided')}>
                      {item.decided_at ? <Ltr>{formatTimestamp(item.decided_at, locale)}</Ltr> : t('common.empty_cell')}
                    </td>
                    <td data-label={t('approvals.col_decided_by')} class="mono">
                      {item.decided_by ?? t('common.empty_cell')}
                    </td>
                    <td data-label={t('approvals.col_audit_ref')} class="mono">
                      {item.audit_ref ?? t('common.empty_cell')}
                    </td>
                    <td data-label={t('approvals.col_result')}>
                      <ResultReference result={item.result ?? null} />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>

      <ConfirmDialog
        open={confirming}
        onClose={() => setConfirming(false)}
        title={confirmTitle}
        confirmLabel={confirmAction}
        onConfirm={() => void approveChosen()}
        busy={bulkBusy}
        tone="human"
        disabled={!plan.canApprove}
      >
        <p>
          {t('approvals.confirm_body', {
            currency: plan.currency ?? t('approvals.own_currency'),
            total: totalText ? t('approvals.confirm_total', { total: totalText }) : '',
          })}
        </p>
      </ConfirmDialog>
    </>
  );
}

/** The approval's own page: the entry preview, the amount, and approve or reject. */
export function ApprovalDetail({ approvalId }: { approvalId: string }) {
  const { client, refreshApprovals } = useSession();
  const state = useAsync(() => approvalGet(client, approvalId), [client.baseUrl, client.apiKey, approvalId]);
  const [reason, setReason] = useState('');
  const [busy, setBusy] = useState<'approve' | 'reject' | null>(null);
  const [notice, setNotice] = useState<{ kind: 'ok' | 'error'; message: string } | null>(null);
  const [outcome, setOutcome] = useState<ApprovalView | null>(null);
  const [outcomeResult, setOutcomeResult] = useState<unknown>(null);

  if (client.apiKey) return <AgentKeyNotice />;
  if (state.loading && !state.data && !outcome) return <Skeleton rows={6} label={t('approvals.loading_detail')} />;
  if (state.error && !state.data && !outcome) {
    return (
      <>
        <a class="back-link" href="#/approvals">
          {t('approvals.back')}
        </a>
        <ErrorBanner error={state.error} onRetry={state.reload} />
      </>
    );
  }
  const item = outcome ?? state.data;
  if (!item) return null;

  const pending = item.status === 'pending';
  const now = Date.now();
  const locale = intlLocale();
  const result = outcome ? outcomeResult : (item.result ?? null);
  const preview = item.preview ?? null;

  async function decide(decision: 'approve' | 'reject') {
    setBusy(decision);
    setNotice(null);
    const trimmed = reason.trim();
    try {
      const output = await approvalDecide(
        client,
        approvalId,
        decision === 'approve' ? { decision: 'approve' } : trimmed ? { decision: 'reject', reason: trimmed } : { decision: 'reject' },
      );
      setOutcome(output.approval);
      setOutcomeResult(output.result ?? null);
      setNotice({ kind: 'ok', message: t(decision === 'approve' ? 'approvals.approved' : 'approvals.rejected') });
      refreshApprovals();
    } catch (error) {
      const failure = classifyDecisionError(error);
      if (failure.kind === 'reauth') {
        setNotice({ kind: 'error', message: t('approvals.reauth') });
        window.location.assign(reauthUrl(client.baseUrl, approvalId));
        return;
      }
      if (failure.kind === 'stale') {
        setOutcome(null);
        setNotice({ kind: 'error', message: staleText(failure.reason) });
        state.reload();
        refreshApprovals();
        return;
      }
      setNotice({ kind: 'error', message: describeError(error).title });
    } finally {
      setBusy(null);
    }
  }

  return (
    <>
      <a class="back-link" href="#/approvals">
        {t('approvals.back')}
      </a>
      <PageHead
        title={pending ? t('approvals.title_pending') : t('approvals.title_decided', { status: t(STATUS_KEY[item.status]) })}
      >
        {pending
          ? t('approvals.detail_pending', {
              when: relativeTime(item.created_at, now, locale),
              key: item.requested_by,
              expiry: expiryPhrase(item.expires_at, now),
            })
          : t('approvals.decided_line', {
              when: item.decided_at ? formatTimestamp(item.decided_at, locale) : t('approvals.decided_by_server'),
            })}
      </PageHead>

      {notice && (
        <div class={`banner ${notice.kind === 'error' ? 'error' : 'human'}`} role={notice.kind === 'error' ? 'alert' : 'status'}>
          <span class="grow">{notice.message}</span>
          {outcome && outcomeResult !== null && resultEntry(outcomeResult) && (
            <span>{tRich('approvals.entry_posted_as', { ref: <ResultReference result={outcomeResult} /> })}</span>
          )}
        </div>
      )}

      <section class="panel" aria-labelledby="request-heading">
        <div class="panel-head">
          <h2 id="request-heading" class="grow">
            {t('approvals.request')}
          </h2>
          <StatusChip status={item.status} />
          <RiskChip risk={item.risk} />
        </div>
        <div class="panel-body">
          <dl class="entry-meta detail-meta">
            <div>
              <dt>{t('term.amount')}</dt>
              <dd class="amount-value">
                <Ltr>{amountText(item.amount, locale)}</Ltr>
              </dd>
            </div>
            <div>
              <dt>{t('approvals.counterparty')}</dt>
              <dd>{item.counterparty ?? t('common.empty_cell')}</dd>
            </div>
            <div>
              <dt>{t('approvals.col_reason')}</dt>
              <dd>{item.reason}</dd>
            </div>
            <div>
              <dt>{t('approvals.col_requested_by')}</dt>
              <dd>
                <AgentKey id={item.requested_by} />
              </dd>
            </div>
            <div>
              <dt>{t('approvals.command')}</dt>
              <dd class="mono-value">{item.command}</dd>
            </div>
            <div>
              <dt>{t('approvals.col_expires')}</dt>
              <dd>
                <Ltr>{formatTimestamp(item.expires_at, locale)}</Ltr>
                <span class="muted detail-sub">{expiryPhrase(item.expires_at, now)}</span>
              </dd>
            </div>
            <div>
              <dt>{t('approvals.request_hash')}</dt>
              <dd class="mono-value" title={item.request_hash}>
                {shortHash(item.request_hash)}
              </dd>
            </div>
          </dl>
        </div>
      </section>

      <section class="panel" aria-labelledby="preview-heading">
        <div class="panel-head">
          <h2 id="preview-heading" class="grow">
            {t('approvals.entry_preview')}
          </h2>
        </div>
        {preview && preview.lines.length > 0 ? (
          <div class="table-wrap">
            <table class="ledger cards">
              <thead>
                <tr>
                  <th scope="col">{t('term.account')}</th>
                  <th scope="col">{t('new.col_side')}</th>
                  <th scope="col" class="num">
                    {t('term.amount')}
                  </th>
                </tr>
              </thead>
              <tbody>
                {preview.lines.map((line, index) => (
                  <tr key={`${line.account}-${index}`}>
                    <td class="code lead" data-label={t('term.account')}>
                      <Ltr>{line.account}</Ltr>
                    </td>
                    <td data-label={t('new.col_side')}>
                      <span class={`side ${line.side}`}>{t(line.side === 'debit' ? 'new.side_debit' : 'new.side_credit')}</span>
                    </td>
                    <td class="num" data-label={t('term.amount')}>
                      <Ltr>{displayAmount(line.amount, line.currency, locale)}</Ltr>
                    </td>
                  </tr>
                ))}
              </tbody>
              {preview.totals && Object.keys(preview.totals).length > 0 && (
                <tfoot>
                  {Object.entries(preview.totals).map(([currency, value]) => (
                    <tr key={currency}>
                      <td colSpan={2}>{t('entry.total', { currency })}</td>
                      <td class="num" data-label={t('term.amount')}>
                        <Ltr>{displayAmount(value, currency, locale)}</Ltr>
                      </td>
                    </tr>
                  ))}
                </tfoot>
              )}
            </table>
          </div>
        ) : (
          <div class="panel-body muted">{t('approvals.no_preview')}</div>
        )}
      </section>

      <section class="panel decision" aria-labelledby="decision-heading">
        <div class="panel-head">
          <h2 id="decision-heading" class="grow">
            {pending ? t('approvals.your_decision') : t('approvals.decision')}
          </h2>
        </div>
        <div class="panel-body">
          {pending ? (
            <div class="decision-form">
              <label class="field" for="decision-reason">
                <span>{t('approvals.reason_optional')}</span>
                <span class="hint">{t('approvals.reason_hint')}</span>
                <textarea
                  id="decision-reason"
                  class="input"
                  rows={3}
                  value={reason}
                  onInput={(event) => setReason((event.target as HTMLTextAreaElement).value)}
                />
              </label>
              <div class="form-actions">
                <button type="button" class="btn btn-human" disabled={busy !== null} onClick={() => void decide('approve')} aria-busy={busy === 'approve' ? 'true' : undefined}>
                  {busy === 'approve' ? t('approvals.approving') : t('approvals.approve')}
                </button>
                <button type="button" class="btn btn-danger" disabled={busy !== null} onClick={() => void decide('reject')} aria-busy={busy === 'reject' ? 'true' : undefined}>
                  {busy === 'reject' ? t('approvals.rejecting') : t('approvals.reject')}
                </button>
              </div>
            </div>
          ) : (
            <dl class="entry-meta detail-meta">
              <div>
                <dt>{t('approvals.decided')}</dt>
                <dd>{item.decided_at ? <Ltr>{formatTimestamp(item.decided_at, locale)}</Ltr> : t('common.empty_cell')}</dd>
              </div>
              <div>
                <dt>{t('approvals.decided_by')}</dt>
                <dd class="mono-value">{item.decided_by ?? t('common.empty_cell')}</dd>
              </div>
              <div>
                <dt>{t('approvals.decision_reason')}</dt>
                <dd>{item.decision_reason ?? t('common.empty_cell')}</dd>
              </div>
              <div>
                <dt>{t('approvals.audit_reference')}</dt>
                <dd class="mono-value">{item.audit_ref ?? t('common.empty_cell')}</dd>
              </div>
              <div>
                <dt>{t('approvals.result')}</dt>
                <dd>
                  <ResultReference result={result} />
                </dd>
              </div>
            </dl>
          )}
        </div>
      </section>
    </>
  );
}

/** The approvals waiting for the person, for the overview. Shown only while the count is above zero. */
export function WaitingForYou({ count }: { count: number }) {
  const { client } = useSession();
  const state = useAsync(() => approvalsList(client, 'pending', 3), [client.baseUrl, client.apiKey, count]);
  const now = Date.now();
  const locale = intlLocale();
  return (
    <section class="panel waiting" aria-labelledby="waiting-heading">
      <div class="panel-head">
        <h2 id="waiting-heading" class="grow">
          {t('approvals.waiting_title')}
        </h2>
        <span class="muted">{t('approvals.pending_count', { count })}</span>
        <a class="btn btn-sm btn-human" href="#/approvals">
          {t('approvals.review')}
        </a>
      </div>
      {state.loading && !state.data ? (
        <div class="panel-body muted">{t('approvals.loading_short')}</div>
      ) : state.error && !state.data ? (
        <div class="panel-body">
          <ErrorBanner error={state.error} onRetry={state.reload} />
        </div>
      ) : (
        <ul class="waiting-list">
          {(state.data?.items ?? []).map((item) => (
            <li key={item.id}>
              <a href={`#/approvals/${encodeURIComponent(item.id)}`}>
                <span class="waiting-main">
                  <strong>{labelOf(item)}</strong>
                  <span class="muted">{item.reason}</span>
                </span>
                <span class="num">
                  <Ltr>{amountText(item.amount, locale)}</Ltr>
                </span>
                <span class="muted waiting-time">{expiryPhrase(item.expires_at, now)}</span>
              </a>
            </li>
          ))}
          {count > (state.data?.items.length ?? 0) && (
            <li class="muted waiting-more">
              <a href="#/approvals">{t('approvals.and_more', { count: count - (state.data?.items.length ?? 0) })}</a>
            </li>
          )}
        </ul>
      )}
    </section>
  );
}
