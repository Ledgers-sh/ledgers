import { useState } from 'preact/hooks';
import {
  fiscalYearClose,
  fiscalYearCreate,
  fiscalYearsList,
  periodClose,
  periodReopen,
  type FiscalYearView,
  type PeriodView,
} from '../../api/settings';
import { useAsync } from '../../hooks';
import { formatPeriod, t, type MessageKey } from '../../i18n';
import { useSession } from '../../session';
import { ConfirmDialog, ErrorBanner, Empty } from '../common';
import { formatWhen } from '../AgentKeys';
import { FormProblem, LoadingLine, SettingsPanel, StateBadge } from './common';

const STATE_LABEL: Record<PeriodView['state'], MessageKey> = {
  open: 'settings.state_open',
  soft_closed: 'settings.state_soft_closed',
  closed: 'settings.state_closed',
};

/** Settings → Accounting year: fiscal years, months that are locked, and the year-end close. */
export function YearSettings() {
  const { client, toast } = useSession();
  const state = useAsync(() => fiscalYearsList(client), [client.baseUrl, client.apiKey]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const [closing, setClosing] = useState<FiscalYearView | null>(null);
  const [closeError, setCloseError] = useState<unknown>(null);
  const [reopening, setReopening] = useState<PeriodView | null>(null);
  const [reopenError, setReopenError] = useState<unknown>(null);
  const [reason, setReason] = useState('');

  if (state.error && !state.data) return <ErrorBanner error={state.error} onRetry={state.reload} />;
  if (!state.data) return <LoadingLine />;
  const years = state.data.years;
  const nextLabel = years.length > 0 ? Math.max(...years.map((year) => year.label)) + 1 : new Date().getUTCFullYear();

  async function run(action: () => Promise<unknown>, done: string, onError: (err: unknown) => void) {
    setBusy(true);
    onError(null);
    try {
      await action();
      toast(done);
      state.reload();
      return true;
    } catch (err) {
      onError(err);
      return false;
    } finally {
      setBusy(false);
    }
  }

  async function openNext() {
    setError(null);
    await run(() => fiscalYearCreate(client, nextLabel), t('settings.year_opened', { year: nextLabel }), setError);
  }

  async function confirmClose() {
    if (!closing) return;
    const ok = await run(() => fiscalYearClose(client, closing.label), t('settings.year_closed', { year: closing.label }), setCloseError);
    if (ok) setClosing(null);
  }

  async function confirmReopen() {
    if (!reopening || reason.trim() === '') return;
    const ok = await run(() => periodReopen(client, reopening.period, reason.trim()), t('settings.month_reopened', { period: formatPeriod(reopening.period) }), setReopenError);
    if (ok) {
      setReopening(null);
      setReason('');
    }
  }

  return (
    <>
      <FormProblem error={error} />
      <div class="settings-toolbar">
        <p class="muted">{t('settings.years_help')}</p>
        <button type="button" class="btn btn-primary" disabled={busy} onClick={() => void openNext()}>
          {t('settings.open_year', { year: nextLabel })}
        </button>
      </div>
      {years.length === 0 ? (
        <Empty title={t('settings.years_empty_title')}>{t('settings.years_empty_body')}</Empty>
      ) : null}
      {[...years].reverse().map((year) => {
        const titleId = `year-${year.label}-title`;
        return (
          <SettingsPanel
            key={year.label}
            id={titleId}
            title={t('settings.year_title', { year: year.label })}
            actions={
              <>
                {year.closed ? (
                  <StateBadge tone="closed">{t('settings.year_closed_at', { when: formatWhen(year.closed_at) })}</StateBadge>
                ) : (
                  <>
                    <StateBadge tone="open">{t('settings.year_open')}</StateBadge>
                    <button
                      type="button"
                      class="btn btn-sm"
                      disabled={busy}
                      onClick={() => {
                        setCloseError(null);
                        setClosing(year);
                      }}
                    >
                      {t('settings.close_year', { year: year.label })}
                    </button>
                  </>
                )}
              </>
            }
          >
            <p class="muted small">{t('settings.year_span', { start: year.starts_on, end: year.ends_on })}</p>
            <div class="table-wrap">
              <table class="settings-table">
                <thead>
                  <tr>
                    <th scope="col">{t('settings.col_month')}</th>
                    <th scope="col">{t('settings.col_state')}</th>
                    <th scope="col">
                      <span class="sr-only">{t('settings.col_actions')}</span>
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {year.periods.map((month) => (
                    <tr key={month.period}>
                      <th scope="row">{formatPeriod(month.period)}</th>
                      <td>
                        <StateBadge tone={month.state === 'closed' ? 'closed' : month.state === 'soft_closed' ? 'soft' : 'open'}>
                          {t(STATE_LABEL[month.state])}
                        </StateBadge>
                      </td>
                      <td class="actions-cell">
                        {year.closed ? null : month.state === 'closed' ? (
                          <button
                            type="button"
                            class="btn btn-sm"
                            disabled={busy}
                            aria-label={t('settings.reopen_named', { month: formatPeriod(month.period) })}
                            onClick={() => {
                              setReopenError(null);
                              setReason('');
                              setReopening(month);
                            }}
                          >
                            {t('settings.reopen')}
                          </button>
                        ) : (
                          <button
                            type="button"
                            class="btn btn-sm"
                            disabled={busy}
                            aria-label={t('settings.lock_named', { month: formatPeriod(month.period) })}
                            onClick={() =>
                              void run(() => periodClose(client, month.period), t('settings.month_locked', { period: formatPeriod(month.period) }), setError)
                            }
                          >
                            {t('settings.lock')}
                          </button>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </SettingsPanel>
        );
      })}

      <ConfirmDialog
        open={closing !== null}
        onClose={() => setClosing(null)}
        title={closing ? t('settings.close_year_title', { year: closing.label }) : ''}
        confirmLabel={closing ? t('settings.close_year_confirm', { year: closing.label }) : ''}
        onConfirm={() => void confirmClose()}
        busy={busy}
        error={closeError}
        danger
      >
        {closing ? (
          <div class="stack">
            <p>{t('settings.close_year_what', { year: closing.label, next: closing.label + 1 })}</p>
            <ol class="settings-steps">
              <li>{t('settings.close_year_step_lock')}</li>
              <li>{t('settings.close_year_step_entry')}</li>
              <li>{t('settings.close_year_step_lasting')}</li>
            </ol>
            <p class="hint">{t('settings.close_year_no_undo')}</p>
          </div>
        ) : null}
      </ConfirmDialog>

      <ConfirmDialog
        open={reopening !== null}
        onClose={() => setReopening(null)}
        title={reopening ? t('settings.reopen_title', { month: formatPeriod(reopening.period) }) : ''}
        confirmLabel={t('settings.reopen')}
        onConfirm={() => void confirmReopen()}
        busy={busy}
        error={reopenError}
        disabled={reason.trim() === ''}
      >
        <div class="stack">
          <p>{t('settings.reopen_body')}</p>
          <label class="field">
            <span>{t('settings.reopen_reason')}</span>
            <input
              class="input"
              name="reason"
              aria-label={t('settings.reopen_reason')}
              value={reason}
              maxLength={512}
              autoComplete="off"
              aria-describedby="reopen-reason-hint"
              onInput={(event) => setReason((event.target as HTMLInputElement).value)}
            />
            <span id="reopen-reason-hint" class="hint">
              {t('settings.reopen_reason_hint')}
            </span>
          </label>
        </div>
      </ConfirmDialog>
    </>
  );
}
