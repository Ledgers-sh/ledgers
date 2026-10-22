import { useEffect, useState } from 'preact/hooks';
import { approvalSettingsGet, approvalSettingsUpdate } from '../../api/settings';
import { useAsync } from '../../hooks';
import { t, type MessageKey } from '../../i18n';
import { useSession } from '../../session';
import { amountProblem, currencyProblem } from '../../lib/vat';
import { ErrorBanner } from '../common';
import { FieldProblem, FormProblem, LoadingLine, SettingsPanel } from './common';

/** Decimal places of the common ISO 4217 currencies; any other code is taken as two. The server checks the exact exponent. */
const ZERO_DECIMALS = new Set(['JPY', 'KRW', 'CLP', 'VND', 'ISK']);
const THREE_DECIMALS = new Set(['BHD', 'KWD', 'JOD', 'OMR', 'TND', 'IQD', 'LYD']);

export function currencyExponent(code: string): number {
  const upper = code.trim().toUpperCase();
  if (ZERO_DECIMALS.has(upper)) return 0;
  if (THREE_DECIMALS.has(upper)) return 3;
  return 2;
}

interface LimitRow {
  id: number;
  code: string;
  amount: string;
}

export const REQUEST_EXPIRY_MIN = 1;
export const REQUEST_EXPIRY_MAX = 168;

/** Settings → Approvals and agent rules: the low-risk limits per currency, how long requests wait, and the fixed sign-in window. */
export function ApprovalSettings() {
  const { client, toast } = useSession();
  const state = useAsync(() => approvalSettingsGet(client), [client.baseUrl, client.apiKey]);
  const [rows, setRows] = useState<LimitRow[]>([]);
  const [nextId, setNextId] = useState(1);
  const [ttl, setTtl] = useState('');
  const [problems, setProblems] = useState<Record<string, MessageKey | null>>({});
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<unknown>(null);

  useEffect(() => {
    if (!state.data) return;
    const entries = Object.entries(state.data.low_risk_limits);
    setRows(entries.map(([code, amount], index) => ({ id: index + 1, code, amount })));
    setNextId(entries.length + 1);
    setTtl(String(state.data.request_ttl_hours));
  }, [state.data]);

  if (state.error && !state.data) return <ErrorBanner error={state.error} onRetry={state.reload} />;
  if (!state.data) return <LoadingLine />;
  const view = state.data;

  function update(id: number, field: 'code' | 'amount', value: string) {
    setRows(rows.map((row) => (row.id === id ? { ...row, [field]: value } : row)));
  }

  function check(): Record<string, MessageKey | null> {
    const found: Record<string, MessageKey | null> = {};
    const codes = new Set<string>();
    for (const row of rows) {
      const code = row.code.trim().toUpperCase();
      if (currencyProblem(code)) found[`code-${row.id}`] = 'settings.currency_format';
      else if (codes.has(code)) found[`code-${row.id}`] = 'settings.currency_repeated';
      codes.add(code);
      if (!found[`code-${row.id}`] && amountProblem(row.amount, currencyExponent(code))) {
        found[`amount-${row.id}`] = 'settings.amount_format';
      }
    }
    const hours = Number(ttl);
    if (!/^\d+$/.test(ttl.trim()) || hours < REQUEST_EXPIRY_MIN || hours > REQUEST_EXPIRY_MAX) found.ttl = 'settings.expiry_range';
    return found;
  }

  async function submit(event: Event) {
    event.preventDefault();
    setError(null);
    const found = check();
    setProblems(found);
    const first = Object.keys(found).find((key) => found[key]);
    if (first) {
      document.getElementById(first === 'ttl' ? 'approval-ttl' : first)?.focus();
      return;
    }
    const limits: Record<string, string> = {};
    for (const row of rows) limits[row.code.trim().toUpperCase()] = row.amount.trim();
    setSaving(true);
    try {
      const next = await approvalSettingsUpdate(client, { low_risk_limits: limits, request_ttl_hours: Number(ttl) });
      toast(t('settings.approvals_saved'));
      setTtl(String(next.request_ttl_hours));
      state.reload();
    } catch (err) {
      setError(err);
    } finally {
      setSaving(false);
    }
  }

  return (
    <>
      <SettingsPanel id="low-risk-title" title={t('settings.low_risk_title')}>
        <form class="settings-form" onSubmit={(event) => void submit(event)} noValidate aria-labelledby="low-risk-title">
          <FormProblem error={error} />
          <p class="hint">{t('settings.low_risk_hint')}</p>
          <div class="table-wrap">
            <table class="settings-table">
              <thead>
                <tr>
                  <th scope="col">{t('settings.col_currency')}</th>
                  <th scope="col">{t('settings.col_low_risk_amount')}</th>
                  <th scope="col">
                    <span class="sr-only">{t('settings.col_actions')}</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {rows.map((row) => {
                  const codeError = problems[`code-${row.id}`];
                  const amountError = problems[`amount-${row.id}`];
                  return (
                    <tr key={row.id}>
                      <td>
                        <input
                          id={`code-${row.id}`}
                          class="input mono"
                          value={row.code}
                          maxLength={3}
                          aria-label={t('settings.currency_row', { n: row.id })}
                          aria-invalid={codeError ? 'true' : undefined}
                          aria-describedby={codeError ? `code-${row.id}-error` : undefined}
                          onInput={(event) => update(row.id, 'code', (event.target as HTMLInputElement).value)}
                        />
                        <FieldProblem id={`code-${row.id}-error`} message={codeError ? t(codeError) : null} />
                      </td>
                      <td>
                        <input
                          id={`amount-${row.id}`}
                          class="input num"
                          inputMode="decimal"
                          value={row.amount}
                          aria-label={t('settings.amount_row', { n: row.id })}
                          aria-invalid={amountError ? 'true' : undefined}
                          aria-describedby={amountError ? `amount-${row.id}-error` : undefined}
                          onInput={(event) => update(row.id, 'amount', (event.target as HTMLInputElement).value)}
                        />
                        <FieldProblem id={`amount-${row.id}-error`} message={amountError ? t(amountError) : null} />
                      </td>
                      <td class="actions-cell">
                        <button
                          type="button"
                          class="btn btn-sm btn-ghost"
                          aria-label={t('settings.remove_currency', { code: row.code || row.id })}
                          onClick={() => setRows(rows.filter((item) => item.id !== row.id))}
                        >
                          {t('settings.remove')}
                        </button>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
          <div class="row-actions">
            <button
              type="button"
              class="btn btn-sm"
              onClick={() => {
                setRows([...rows, { id: nextId, code: '', amount: '' }]);
                setNextId(nextId + 1);
              }}
            >
              {t('settings.add_currency')}
            </button>
          </div>

          <label class="field">
            <span>{t('settings.request_expiry')}</span>
            <input
              id="approval-ttl"
              aria-label={t('settings.request_expiry')}
              class="input num"
              type="number"
              inputMode="numeric"
              min={REQUEST_EXPIRY_MIN}
              max={REQUEST_EXPIRY_MAX}
              value={ttl}
              aria-invalid={problems.ttl ? 'true' : undefined}
              aria-describedby={problems.ttl ? 'approval-ttl-error' : 'approval-ttl-hint'}
              onInput={(event) => setTtl((event.target as HTMLInputElement).value)}
            />
            <span id="approval-ttl-hint" class="hint">
              {t('settings.request_expiry_hint')}
            </span>
            <FieldProblem id="approval-ttl-error" message={problems.ttl ? t(problems.ttl) : null} />
          </label>

          <div class="form-actions">
            <button type="submit" class="btn btn-primary" disabled={saving}>
              {t('settings.save')}
            </button>
          </div>
        </form>
      </SettingsPanel>

      <SettingsPanel id="sign-in-title" title={t('settings.sign_in_window_title')}>
        <p>{t('settings.sign_in_window', { minutes: view.fresh_sign_in_minutes })}</p>
        <p class="hint">{t('settings.sign_in_window_why')}</p>
      </SettingsPanel>

      <SettingsPanel id="agent-rules-title" title={t('settings.agent_rules_title')}>
        <p>{t('settings.agent_rules_body')}</p>
        <div class="row-actions">
          <a class="btn" href="#/settings/agent-keys">
            {t('settings.agent_rules_link')}
          </a>
        </div>
      </SettingsPanel>
    </>
  );
}
