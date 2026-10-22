import { useState } from 'preact/hooks';
import { approvalDecide, approvalGet, approvalsBulkApprove, approvalsList, type ApprovalStatus, type ApprovalView } from '../api/approvals';
import { useAsync } from '../hooks';
import { useSession } from '../session';
import { formatTimestamp, shortHash, relativeTime, expiryText, amountText, bulkPlan, bulkTotalMinor, classifyDecisionError, displayAmount, isBulkEligible, reauthUrl, resultEntry, BULK_HIGH_RISK_MESSAGE } from '../lib/approvals';
import { ConfirmDialog, Empty, ErrorBanner, LOCALE, PageHead, Skeleton, describeError } from './common';

const STATUS_LABEL: Record<ApprovalStatus, string> = {
  pending: 'Pending',
  approved: 'Approved',
  rejected: 'Rejected',
  expired: 'Expired',
  superseded: 'Superseded',
};

/** The agent that asked, in agent blue. */
export function AgentKey({ id }: { id: string }) {
  return (
    <span class="agent-key" title="Requesting agent key">
      {id}
    </span>
  );
}

export function RiskChip({ risk }: { risk: ApprovalView['risk'] }) {
  return <span class={`chip risk-${risk}`}>{risk === 'high' ? 'High risk' : 'Low risk'}</span>;
}

export function StatusChip({ status }: { status: ApprovalStatus }) {
  return <span class={`chip status-${status}`}>{STATUS_LABEL[status]}</span>;
}

/** Points at the journal entry a decision posted, or shows the command output as sent. */
function ResultReference({ result }: { result: unknown }) {
  if (result === null || result === undefined) return <span class="muted">None</span>;
  const entry = resultEntry(result);
  if (entry) {
    return (
      <a class="row-link" href={`#/journal/${encodeURIComponent(entry.journalKey)}`}>
        {entry.entryId}
      </a>
    );
  }
  return <pre class="result-json">{JSON.stringify(result, null, 2)}</pre>;
}

function AgentKeyNotice() {
  return (
    <Empty title="Approvals need a person sign-in">
      Agent keys cannot review approvals. Sign in with your passkey to see the requests waiting for you.
    </Empty>
  );
}

function labelOf(item: ApprovalView): string {
  return item.counterparty ?? item.command;
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
  // Only rows still on screen and still eligible can be selected: a reload drops the rest.
  const chosen = new Set([...selected].filter((id) => items.some((item) => item.id === id && isBulkEligible(item))));
  const plan = bulkPlan(items, chosen);
  const total = plan.currency ? bulkTotalMinor(items, chosen, plan.currency) : null;
  const totalText = plan.currency && total !== null ? displayAmount(total, plan.currency, LOCALE) : null;

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
      setNotice({ kind: 'ok', message: `Approved ${count} ${count === 1 ? 'request' : 'requests'}.` });
    } catch (error) {
      const failure = classifyDecisionError(error);
      setNotice({
        kind: 'error',
        message: failure.kind === 'refused' || failure.kind === 'stale' ? failure.message : describeError(error).title,
      });
    } finally {
      setBulkBusy(false);
      state.reload();
      refreshApprovals();
    }
  }

  return (
    <>
      <PageHead title="Approvals">
        Entries an agent cannot post on its own wait here. Nothing is posted until you approve it.
      </PageHead>

      <div class="tabs" role="tablist" aria-label="Approval views">
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
            {name === 'pending' ? 'Pending' : 'History'}
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
            <Skeleton rows={4} label="Loading approvals" />
          </div>
        ) : state.error && !loaded ? (
          <div class="panel-body">
            <ErrorBanner error={state.error} onRetry={state.reload} />
          </div>
        ) : tab === 'pending' ? (
          items.length === 0 ? (
            <div class="panel-body">
              <Empty title="Nothing is waiting for you">Requests from agents that need your approval appear here.</Empty>
            </div>
          ) : (
            <>
              {chosen.size > 0 && (
                <div class="bulk-bar" role="region" aria-label="Bulk approval">
                  <span>
                    <strong>{plan.count}</strong> selected{totalText ? ` · ${totalText}` : ''}
                  </span>
                  {plan.message && (
                    <span class="bulk-message" role="status">
                      {plan.message}
                    </span>
                  )}
                  <span class="grow" />
                  <button type="button" class="btn btn-human" disabled={!plan.canApprove || bulkBusy} onClick={() => setConfirming(true)}>
                    Approve selected ({plan.count})
                  </button>
                  <button type="button" class="btn btn-ghost btn-sm" onClick={() => setSelected(new Set())}>
                    Clear
                  </button>
                </div>
              )}
              <div class="table-wrap">
                <table class="ledger cards">
                  <thead>
                    <tr>
                      <th scope="col">
                        <span class="sr-only">Select for bulk approval</span>
                      </th>
                      <th scope="col">Counterparty</th>
                      <th scope="col" class="num">
                        Amount
                      </th>
                      <th scope="col">Reason</th>
                      <th scope="col">Requested by</th>
                      <th scope="col">Risk</th>
                      <th scope="col">Expires</th>
                    </tr>
                  </thead>
                  <tbody>
                    {items.map((item) => {
                      const eligible = isBulkEligible(item);
                      return (
                        <tr key={item.id}>
                          <td class="check-cell" data-label="Select">
                            <input
                              type="checkbox"
                              aria-label={`Select ${labelOf(item)} for bulk approval`}
                              checked={chosen.has(item.id)}
                              disabled={!eligible || bulkBusy}
                              title={eligible ? undefined : BULK_HIGH_RISK_MESSAGE}
                              onChange={(event) => toggle(item.id, (event.target as HTMLInputElement).checked)}
                            />
                          </td>
                          <td class="lead" data-label="Counterparty">
                            <a class="row-link" href={`#/approvals/${encodeURIComponent(item.id)}`}>
                              {labelOf(item)}
                            </a>
                          </td>
                          <td class="num" data-label="Amount">
                            {amountText(item.amount, LOCALE)}
                          </td>
                          <td data-label="Reason">{item.reason}</td>
                          <td data-label="Requested by">
                            <AgentKey id={item.requested_by} />
                          </td>
                          <td data-label="Risk">
                            <RiskChip risk={item.risk} />
                          </td>
                          <td data-label="Expires">{expiryText(item.expires_at, now)}</td>
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
            <Empty title="No decisions yet">Approved, rejected and expired requests are listed here with their audit reference.</Empty>
          </div>
        ) : (
          <div class="table-wrap">
            <table class="ledger cards">
              <thead>
                <tr>
                  <th scope="col">Status</th>
                  <th scope="col">Counterparty</th>
                  <th scope="col" class="num">
                    Amount
                  </th>
                  <th scope="col">Decided</th>
                  <th scope="col">Decided by</th>
                  <th scope="col">Audit ref</th>
                  <th scope="col">Result</th>
                </tr>
              </thead>
              <tbody>
                {items.map((item) => (
                  <tr key={item.id}>
                    <td data-label="Status">
                      <StatusChip status={item.status} />
                    </td>
                    <td class="lead" data-label="Counterparty">
                      <a class="row-link" href={`#/approvals/${encodeURIComponent(item.id)}`}>
                        {labelOf(item)}
                      </a>
                    </td>
                    <td class="num" data-label="Amount">
                      {amountText(item.amount, LOCALE)}
                    </td>
                    <td data-label="Decided">{item.decided_at ? formatTimestamp(item.decided_at, LOCALE) : '—'}</td>
                    <td data-label="Decided by" class="mono">
                      {item.decided_by ?? '—'}
                    </td>
                    <td data-label="Audit ref" class="mono">
                      {item.audit_ref ?? '—'}
                    </td>
                    <td data-label="Result">
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
        title={`Approve ${plan.count} ${plan.count === 1 ? 'request' : 'requests'}?`}
        confirmLabel={`Approve ${plan.count} ${plan.count === 1 ? 'request' : 'requests'}`}
        onConfirm={() => void approveChosen()}
        busy={bulkBusy}
        tone="human"
        disabled={!plan.canApprove}
      >
        <p>
          Each request posts its entry in {plan.currency ?? 'its own currency'}
          {totalText ? `, a total of ${totalText}` : ''}. Only low-risk requests are in this selection.
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
  if (state.loading && !state.data && !outcome) return <Skeleton rows={6} label="Loading approval" />;
  if (state.error && !state.data && !outcome) {
    return (
      <>
        <a class="back-link" href="#/approvals">
          ← Approvals
        </a>
        <ErrorBanner error={state.error} onRetry={state.reload} />
      </>
    );
  }
  const item = outcome ?? state.data;
  if (!item) return null;

  const pending = item.status === 'pending';
  const now = Date.now();
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
      setNotice({
        kind: 'ok',
        message: decision === 'approve' ? 'Approved.' : 'Rejected. Nothing was posted.',
      });
      refreshApprovals();
    } catch (error) {
      const failure = classifyDecisionError(error);
      if (failure.kind === 'reauth') {
        setNotice({
          kind: 'error',
          message: 'Approving a high-risk request needs a fresh sign-in. Signing you in again; you will return to this request.',
        });
        window.location.assign(reauthUrl(client.baseUrl, approvalId));
        return;
      }
      if (failure.kind === 'stale') {
        setOutcome(null);
        setNotice({ kind: 'error', message: failure.message });
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
        ← Approvals
      </a>
      <PageHead title={pending ? 'Approval request' : `${STATUS_LABEL[item.status]} request`}>
        {pending
          ? `Requested ${relativeTime(item.created_at, now)} by agent key ${item.requested_by}. ${expiryText(item.expires_at, now)}.`
          : `Decided ${item.decided_at ? formatTimestamp(item.decided_at, LOCALE) : 'by the server'}.`}
      </PageHead>

      {notice && (
        <div class={`banner ${notice.kind === 'error' ? 'error' : 'human'}`} role={notice.kind === 'error' ? 'alert' : 'status'}>
          <span class="grow">{notice.message}</span>
          {outcome && outcomeResult !== null && resultEntry(outcomeResult) && (
            <span>
              Entry posted as <ResultReference result={outcomeResult} />
            </span>
          )}
        </div>
      )}

      <section class="panel" aria-labelledby="request-heading">
        <div class="panel-head">
          <h2 id="request-heading" class="grow">
            Request
          </h2>
          <StatusChip status={item.status} />
          <RiskChip risk={item.risk} />
        </div>
        <div class="panel-body">
          <dl class="entry-meta detail-meta">
            <div>
              <dt>Amount</dt>
              <dd class="amount-value">{amountText(item.amount, LOCALE)}</dd>
            </div>
            <div>
              <dt>Counterparty</dt>
              <dd>{item.counterparty ?? '—'}</dd>
            </div>
            <div>
              <dt>Reason</dt>
              <dd>{item.reason}</dd>
            </div>
            <div>
              <dt>Requested by</dt>
              <dd>
                <AgentKey id={item.requested_by} />
              </dd>
            </div>
            <div>
              <dt>Command</dt>
              <dd class="mono-value">{item.command}</dd>
            </div>
            <div>
              <dt>Expires</dt>
              <dd>
                {formatTimestamp(item.expires_at, LOCALE)}
                <span class="muted detail-sub">{expiryText(item.expires_at, now)}</span>
              </dd>
            </div>
            <div>
              <dt>Request hash</dt>
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
            Entry preview
          </h2>
        </div>
        {preview && preview.lines.length > 0 ? (
          <div class="table-wrap">
            <table class="ledger cards">
              <thead>
                <tr>
                  <th scope="col">Account</th>
                  <th scope="col">Side</th>
                  <th scope="col" class="num">
                    Amount
                  </th>
                </tr>
              </thead>
              <tbody>
                {preview.lines.map((line, index) => (
                  <tr key={`${line.account}-${index}`}>
                    <td class="code lead" data-label="Account">
                      {line.account}
                    </td>
                    <td data-label="Side">
                      <span class={`side ${line.side}`}>{line.side === 'debit' ? 'Debit' : 'Credit'}</span>
                    </td>
                    <td class="num" data-label="Amount">
                      {displayAmount(line.amount, line.currency, LOCALE)}
                    </td>
                  </tr>
                ))}
              </tbody>
              {preview.totals && Object.keys(preview.totals).length > 0 && (
                <tfoot>
                  {Object.entries(preview.totals).map(([currency, value]) => (
                    <tr key={currency}>
                      <td colSpan={2}>Total {currency}</td>
                      <td class="num" data-label="Total">
                        {displayAmount(value, currency, LOCALE)}
                      </td>
                    </tr>
                  ))}
                </tfoot>
              )}
            </table>
          </div>
        ) : (
          <div class="panel-body muted">No preview was stored for this request.</div>
        )}
      </section>

      <section class="panel decision" aria-labelledby="decision-heading">
        <div class="panel-head">
          <h2 id="decision-heading" class="grow">
            {pending ? 'Your decision' : 'Decision'}
          </h2>
        </div>
        <div class="panel-body">
          {pending ? (
            <div class="decision-form">
              <label class="field" for="decision-reason">
                <span>Reason (optional)</span>
                <span class="hint">Kept with the decision in the audit trail.</span>
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
                  {busy === 'approve' ? 'Approving…' : 'Approve'}
                </button>
                <button type="button" class="btn btn-danger" disabled={busy !== null} onClick={() => void decide('reject')} aria-busy={busy === 'reject' ? 'true' : undefined}>
                  {busy === 'reject' ? 'Rejecting…' : 'Reject'}
                </button>
              </div>
            </div>
          ) : (
            <dl class="entry-meta detail-meta">
              <div>
                <dt>Decided</dt>
                <dd>{item.decided_at ? formatTimestamp(item.decided_at, LOCALE) : '—'}</dd>
              </div>
              <div>
                <dt>Decided by</dt>
                <dd class="mono-value">{item.decided_by ?? '—'}</dd>
              </div>
              <div>
                <dt>Decision reason</dt>
                <dd>{item.decision_reason ?? '—'}</dd>
              </div>
              <div>
                <dt>Audit reference</dt>
                <dd class="mono-value">{item.audit_ref ?? '—'}</dd>
              </div>
              <div>
                <dt>Result</dt>
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
  return (
    <section class="panel waiting" aria-labelledby="waiting-heading">
      <div class="panel-head">
        <h2 id="waiting-heading" class="grow">
          Waiting for you
        </h2>
        <span class="muted">{count} pending</span>
        <a class="btn btn-sm btn-human" href="#/approvals">
          Review approvals
        </a>
      </div>
      {state.loading && !state.data ? (
        <div class="panel-body muted">Loading approvals…</div>
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
                <span class="num">{amountText(item.amount, LOCALE)}</span>
                <span class="muted waiting-time">{expiryText(item.expires_at, now)}</span>
              </a>
            </li>
          ))}
          {count > (state.data?.items.length ?? 0) && (
            <li class="muted waiting-more">
              <a href="#/approvals">and {count - (state.data?.items.length ?? 0)} more</a>
            </li>
          )}
        </ul>
      )}
    </section>
  );
}
