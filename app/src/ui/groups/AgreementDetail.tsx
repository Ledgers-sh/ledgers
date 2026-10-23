/**
 * One agreement: its terms, the withholding and the accounts this company
 * names, and the period timeline. A month moves from draft (run) to posted in
 * both companies, then settled. A failed post shows the server's reason and
 * can be retried. The post and settle dialogs show both sides' entries side
 * by side before anything is written.
 */
import { useState } from 'preact/hooks';
import {
  agreementGet,
  agreementTerminate,
  postPeriods,
  runPeriods,
  settlePeriods,
  type AgreementKind,
  type AgreementView,
  type DocStatus,
  type PeriodDocView,
} from '../../api/groups';
import { useAsync } from '../../hooks';
import { intlLocale, formatPeriod, t, type MessageKey } from '../../i18n';
import {
  accrualPreview,
  basisText,
  canChangeSettings,
  canMoveMoney,
  canPostPeriods,
  formatRateBp,
  nextAction,
  periodsBetween,
  settlementPreview,
  timelineCells,
  whtDisplay,
  type PreviewSide,
  type TimelineCell,
} from '../../lib/groups';
import type { MonthRange } from '../../lib/trends';
import { ConfirmDialog, ErrorBanner, Money, PageHead, Skeleton } from '../common';
import { PeriodPicker, rangeLabel } from '../PeriodPicker';
import { useGroupContext, type GroupContext } from './context';
import { CompanyName, GroupTabs, RoleNotice } from './shared';
import { partiesOf } from './Agreements';

const KIND_KEY: Record<AgreementKind, MessageKey> = {
  licence: 'agreements.kind.licence',
  royalty: 'agreements.kind.royalty',
  management_fee: 'agreements.kind.management_fee',
  loan: 'agreements.kind.loan',
};

const STATUS_KEY: Record<DocStatus, MessageKey> = {
  draft: 'agreements.doc.draft',
  posted: 'agreements.doc.posted',
  settled: 'agreements.doc.settled',
  failed: 'agreements.doc.failed',
};

const STATE_KEY: Record<TimelineCell['state'], MessageKey> = {
  none: 'agreements.timeline.none',
  draft: 'agreements.doc.draft',
  posted: 'agreements.doc.posted',
  settled: 'agreements.doc.settled',
  failed: 'agreements.doc.failed',
};

type ActionKind = 'post' | 'settle' | 'retry';

/** The current month in UTC, the way the server numbers periods. */
function currentMonth(): string {
  return new Date().toISOString().slice(0, 7);
}

export function AgreementDetail({ agreementId }: { agreementId: string }) {
  const ctx = useGroupContext();
  const detail = useAsync(() => agreementGet(ctx.client, agreementId), [ctx.client.baseUrl, agreementId]);
  if (detail.error !== undefined) {
    return (
      <>
        <PageHead title={t('agreements.title')} />
        <GroupTabs current="agreements" />
        <ErrorBanner error={detail.error} onRetry={detail.reload} />
      </>
    );
  }
  if (!detail.data) return <Skeleton rows={4} label={t('agreements.loading')} />;
  return (
    <AgreementBody ctx={ctx} agreement={detail.data.agreement} periods={detail.data.periods} reload={detail.reload} />
  );
}

function AgreementBody({
  ctx,
  agreement,
  periods,
  reload,
}: {
  ctx: GroupContext;
  agreement: AgreementView;
  periods: PeriodDocView[];
  reload: () => void;
}) {
  const group = ctx.groups?.find((item) => item.id === agreement.group_id);
  const parties = partiesOf(agreement, group, ctx);
  const canSettings = canChangeSettings(ctx.role);
  const canPost = canPostPeriods(ctx.role);
  const canMoney = canMoveMoney(ctx.role);
  const terminated = agreement.terminated_at !== null;
  const cells = timelineCells(periods);
  const [range, setRange] = useState<MonthRange>({ from: currentMonth(), to: currentMonth() });
  const [runBusy, setRunBusy] = useState(false);
  const [runError, setRunError] = useState<unknown>(null);
  const [dialog, setDialog] = useState<{ kind: ActionKind; cell: TimelineCell } | null>(null);
  const [actionBusy, setActionBusy] = useState(false);
  const [actionError, setActionError] = useState<unknown>(null);
  const [endBusy, setEndBusy] = useState(false);
  const [endError, setEndError] = useState<unknown>(null);
  const locale = intlLocale();
  const wht = whtDisplay(agreement);
  const runMonths = periodsBetween(range.from, range.to);

  const run = async () => {
    setRunBusy(true);
    setRunError(null);
    try {
      await runPeriods(ctx.client, agreement.id, runMonths);
    } catch (caught) {
      setRunError(caught);
    } finally {
      setRunBusy(false);
      reload();
    }
  };

  const act = async (kind: ActionKind, period: string): Promise<void> => {
    const call = kind === 'settle' ? settlePeriods : postPeriods;
    setActionBusy(true);
    setActionError(null);
    try {
      await call(ctx.client, agreement.id, [period]);
      setDialog(null);
    } catch (caught) {
      // The refusal names the reason and the month now shows as failed; the dialog stays open to show it.
      setActionError(caught);
    } finally {
      setActionBusy(false);
      reload();
    }
  };

  const end = async () => {
    setEndBusy(true);
    setEndError(null);
    try {
      await agreementTerminate(ctx.client, agreement.id);
    } catch (caught) {
      setEndError(caught);
    } finally {
      setEndBusy(false);
      reload();
    }
  };

  return (
    <>
      <PageHead title={t(KIND_KEY[agreement.kind])}>
        <span class="parties-line">
          <CompanyName name={parties.licensor} hidden={parties.licensor === null} />
          <span aria-hidden="true"> → </span>
          <CompanyName name={parties.licensee} hidden={parties.licensee === null} />
        </span>
      </PageHead>
      <GroupTabs current="agreements" />
      <div class="row-actions">
        <a class="btn btn-sm" href="#/group/agreements">
          {t('agreements.back')}
        </a>
        {canSettings && !terminated && (
          <a class="btn btn-sm" href={`#/group/agreements/${encodeURIComponent(agreement.id)}/edit`}>
            {t('agreements.edit')}
          </a>
        )}
        {terminated && <span class="chip status-rejected">{t('agreements.terminated')}</span>}
      </div>
      {!canSettings && <RoleNotice>{t('agreements.reason_settings')}</RoleNotice>}

      <section class="panel" aria-labelledby="agreement-terms-title">
        <div class="panel-head">
          <h2 id="agreement-terms-title">{t('agreements.terms')}</h2>
        </div>
        <div class="panel-body">
          <dl class="terms">
            <div>
              <dt>{t('agreements.col.basis')}</dt>
              <dd>{t(basisText(agreement.basis))}</dd>
            </div>
            <div>
              <dt>{t('agreements.col.rate')}</dt>
              <dd class="num">
                {agreement.basis === 'fixed_monthly' && agreement.fixed_minor !== null ? (
                  <Money minor={String(agreement.fixed_minor)} currency={agreement.currency} />
                ) : (
                  formatRateBp(agreement.rate_bp, locale)
                )}
              </dd>
            </div>
            <div>
              <dt>{t('groups.wht.label')}</dt>
              <dd>
                <span class="num">{formatRateBp(agreement.wht_bp, locale)}</span> <span class="muted">{t(wht.key)}</span>
              </dd>
            </div>
            <div>
              <dt>{t('agreements.currency')}</dt>
              <dd>{agreement.currency}</dd>
            </div>
            <div>
              <dt>{t('agreements.my_accounts')}</dt>
              <dd>
                <ul class="account-list">
                  <li>
                    {t('agreements.acc.royalty')}: <span class="num">{agreement.accounts.royalty}</span>
                  </li>
                  <li>
                    {t('agreements.acc.ic')}: <span class="num">{agreement.accounts.ic}</span>
                  </li>
                  {agreement.accounts.wht && (
                    <li>
                      {t('agreements.acc.wht')}: <span class="num">{agreement.accounts.wht}</span>
                    </li>
                  )}
                  <li>
                    {t('agreements.acc.bank')}: <span class="num">{agreement.accounts.bank}</span>
                  </li>
                </ul>
              </dd>
            </div>
            {agreement.notes && (
              <div class="terms-wide">
                <dt>{t('agreements.notes')}</dt>
                <dd>{agreement.notes}</dd>
              </div>
            )}
          </dl>
        </div>
      </section>

      <section class="panel timeline-panel" aria-labelledby="timeline-title">
        <div class="panel-head">
          <h2 id="timeline-title">{t('agreements.timeline')}</h2>
        </div>
        <div class="panel-body">
          <div class="run-form">
            <div class="field">
              <span id="run-months-label">{t('agreements.run_months')}</span>
              <PeriodPicker range={range} onChange={setRange} />
            </div>
            <div class="row-actions">
              <button
                type="button"
                class="btn btn-primary"
                disabled={!canPost || runBusy || terminated || runMonths.length === 0}
                aria-describedby="run-summary"
                onClick={() => void run()}
              >
                {t('agreements.run_period')}
              </button>
              <span id="run-summary" class="muted">
                {t('agreements.run_summary', { range: rangeLabel(range), count: runMonths.length })}
              </span>
            </div>
            {!canPost && <RoleNotice>{t('agreements.reason_post')}</RoleNotice>}
            {runError !== null && <ErrorBanner error={runError} />}
          </div>
          {cells.length === 0 ? (
            <p class="muted">{t('agreements.no_periods')}</p>
          ) : (
            <ol class="timeline" aria-label={t('agreements.timeline')}>
              {cells.map((cell) => (
                <TimelineRow
                  key={cell.period}
                  cell={cell}
                  canPost={canPost}
                  canMoney={canMoney}
                  terminated={terminated}
                  onAction={(kind) => {
                    setActionError(null);
                    setDialog({ kind, cell });
                  }}
                />
              ))}
            </ol>
          )}
        </div>
      </section>

      {dialog && (
        <ConfirmDialog
          open
          onClose={() => setDialog(null)}
          title={dialogTitle(dialog.kind, dialog.cell.period)}
          confirmLabel={dialogConfirm(dialog.kind)}
          busy={actionBusy}
          error={actionError ?? undefined}
          onConfirm={() => void act(dialog.kind, dialog.cell.period)}
        >
          <PeriodDialogBody kind={dialog.kind} cell={dialog.cell} bank={agreement.accounts.bank} />
        </ConfirmDialog>
      )}

      <section class="panel" aria-labelledby="end-title">
        <div class="panel-head">
          <h2 id="end-title">{t('agreements.end_title')}</h2>
        </div>
        <div class="panel-body">
          <p class="muted">{t('agreements.end_body')}</p>
          {endError !== null && <ErrorBanner error={endError} />}
          <button type="button" class="btn" disabled={!canSettings || terminated || endBusy} onClick={() => void end()}>
            {t('agreements.end')}
          </button>
        </div>
      </section>
    </>
  );
}

function dialogTitle(kind: ActionKind, period: string): string {
  const label = formatPeriod(period);
  if (kind === 'settle') return t('agreements.settle_title', { period: label });
  if (kind === 'retry') return t('agreements.retry_title', { period: label });
  return t('agreements.post_title', { period: label });
}

function dialogConfirm(kind: ActionKind): string {
  if (kind === 'settle') return t('agreements.settle');
  if (kind === 'retry') return t('agreements.retry');
  return t('agreements.post_both');
}

function TimelineRow({
  cell,
  canPost,
  canMoney,
  terminated,
  onAction,
}: {
  cell: TimelineCell;
  canPost: boolean;
  canMoney: boolean;
  terminated: boolean;
  onAction: (kind: ActionKind) => void;
}) {
  const action = nextAction(cell);
  const allowed = action === 'settle' ? canMoney : canPost;
  const failures = [cell.licensor, cell.licensee].filter((doc): doc is PeriodDocView => doc?.last_error != null);
  const reasonId = `timeline-reason-${cell.period}`;
  return (
    <li class={`timeline-row state-${cell.state}`}>
      <div class="timeline-month">
        <strong>{formatPeriod(cell.period)}</strong>
        <span class={`chip state-${cell.state}`}>{t(STATE_KEY[cell.state])}</span>
      </div>
      <dl class="timeline-sides">
        <div>
          <dt>{t('agreements.licensor')}</dt>
          <dd>{cell.licensor ? t(STATUS_KEY[cell.licensor.status]) : t('agreements.timeline.none')}</dd>
        </div>
        <div>
          <dt>{t('agreements.licensee')}</dt>
          <dd>{cell.licensee ? t(STATUS_KEY[cell.licensee.status]) : t('agreements.timeline.none')}</dd>
        </div>
      </dl>
      {failures.map((doc) => (
        <p key={doc.id} class="timeline-failure" role="alert">
          {t('agreements.failure_reason', { side: t(doc.side === 'licensor' ? 'agreements.licensor' : 'agreements.licensee') })}{' '}
          {doc.last_error}
        </p>
      ))}
      {action !== 'none' && !terminated && (
        <div class="row-actions">
          <button
            type="button"
            class={`btn btn-sm${action === 'settle' ? '' : ' btn-primary'}`}
            disabled={!allowed}
            aria-describedby={allowed ? undefined : reasonId}
            onClick={() => onAction(action)}
          >
            {action === 'settle'
              ? t('agreements.settle')
              : action === 'retry'
                ? t('agreements.retry')
                : t('agreements.review_post')}
            <span class="sr-only"> {formatPeriod(cell.period)}</span>
          </button>
          {!allowed && (
            <span id={reasonId} class="muted">
              {t(action === 'settle' ? 'agreements.reason_settle' : 'agreements.reason_post')}
            </span>
          )}
        </div>
      )}
    </li>
  );
}

function PeriodDialogBody({ kind, cell, bank }: { kind: ActionKind; cell: TimelineCell; bank: string }) {
  if (kind === 'settle') {
    return (
      <>
        <p>{t('agreements.settle_body')}</p>
        <p class="muted">{t('agreements.settle_bank', { bank })}</p>
        <PreviewPair
          preview={settlementPreview(cell.licensor, cell.licensee, cell.period)}
          titles={[t('preview.licensor_receipt'), t('preview.licensee_payment')]}
        />
      </>
    );
  }
  return (
    <>
      <p>{kind === 'retry' ? t('agreements.retry_body') : t('agreements.post_body')}</p>
      {cell.licensee?.last_error && kind === 'retry' && <p class="banner error" role="alert">{cell.licensee.last_error}</p>}
      <PreviewPair preview={accrualPreview(cell.licensor, cell.licensee, cell.period)} />
    </>
  );
}

const LINE_KEY: Record<string, MessageKey> = {
  ic_receivable: 'preview.line.ic_receivable',
  royalty_income: 'preview.line.royalty_income',
  royalty_expense: 'preview.line.royalty_expense',
  ic_payable: 'preview.line.ic_payable',
  wht_payable: 'preview.line.wht_payable',
  bank: 'preview.line.bank',
  wht_receivable: 'preview.line.wht_receivable',
};

/** Both sides' entries, side by side: the licensor's invoice and the licensee's bill with the withholding split. */
export function PreviewPair({
  preview,
  titles,
}: {
  preview: { licensor: PreviewSide; licensee: PreviewSide };
  /** The two cards' titles: the invoice and bill by default, the receipt and payment when settling. */
  titles?: [string, string];
}) {
  return (
    <div class="preview-pair">
      <PreviewCard title={titles?.[0] ?? t('preview.licensor_invoice')} side={preview.licensor} />
      <PreviewCard title={titles?.[1] ?? t('preview.licensee_bill')} side={preview.licensee} />
    </div>
  );
}

function PreviewCard({ title, side }: { title: string; side: PreviewSide }) {
  return (
    <section class="preview-card" aria-label={title}>
      <h3>{title}</h3>
      <table class="preview-table">
        <caption class="sr-only">{title}</caption>
        <thead>
          <tr>
            <th scope="col">{t('preview.account')}</th>
            <th scope="col" class="num">
              {t('preview.debit')}
            </th>
            <th scope="col" class="num">
              {t('preview.credit')}
            </th>
          </tr>
        </thead>
        <tbody>
          {side.lines.map((line, index) => (
            <tr key={`${line.role}-${index}`}>
              <td>{t(LINE_KEY[line.role] ?? 'preview.line.bank')}</td>
              <td class="num">{line.side === 'debit' ? <Money minor={String(line.minor)} currency={side.currency} /> : ''}</td>
              <td class="num">{line.side === 'credit' ? <Money minor={String(line.minor)} currency={side.currency} /> : ''}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <p class="preview-foot">
        <span class={`chip ${side.balanced ? 'status-approved' : 'status-rejected'}`}>
          {side.balanced ? t('preview.balanced') : t('preview.unbalanced')}
        </span>{' '}
        <span class="muted">{t('preview.headline')}</span> <Money minor={String(side.headline)} currency={side.currency} />
      </p>
    </section>
  );
}
