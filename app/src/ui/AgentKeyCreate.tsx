import { useEffect, useRef, useState } from 'preact/hooks';
import { keysCreate } from '../api/commands';
import type { KeysCreateOutput } from '../api/types';
import { limitRowError, limitsPayload, TIERS, type LimitProblem, type LimitRow, type TierValue } from '../lib/agentKeys';
import { useSession } from '../session';
import { t, type MessageKey } from '../i18n';
import { ErrorBanner } from './common';
import { UsageSnippets } from './UsageSnippets';
import { Select } from './Select';
import { colourOf } from '../lib/units';

const NAME_MAX = 80;

/** A limit row's problem, in the reader's language. */
function limitProblemText(problem: LimitProblem): string {
  if (problem.kind === 'currency') return t('agentKeys.limit_currency');
  const key: MessageKey = problem.kind === 'per-entry' ? 'agentKeys.limit_per_entry' : 'agentKeys.limit_per_day';
  return t(key, { currency: problem.currency, decimals: problem.decimals });
}

/**
 * The create dialog: a name, a tier, and per-currency limits. Nothing is sent
 * until the form is valid; a server refusal shows in the dialog.
 */
export function AgentKeyCreate({
  open,
  onClose,
  onCreated,
}: {
  open: boolean;
  onClose: () => void;
  onCreated: (output: KeysCreateOutput) => void;
}) {
  const { client, currency, units } = useSession();
  const ref = useRef<HTMLDialogElement>(null);
  const [name, setName] = useState('');
  const [tier, setTier] = useState<TierValue>('post-within-limits');
  const [rows, setRows] = useState<LimitRow[]>(() => [{ currency, perEntry: '', perDay: '' }]);
  const [problems, setProblems] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  // The business unit the key is pinned to; empty means every unit.
  const [unitPin, setUnitPin] = useState('');

  useEffect(() => {
    const dialog = ref.current;
    if (!dialog) return;
    if (open && !dialog.open) {
      setName('');
      setTier('post-within-limits');
      setUnitPin('');
      setRows([{ currency, perEntry: '', perDay: '' }]);
      setProblems([]);
      setError(null);
      dialog.showModal();
    }
    if (!open && dialog.open) dialog.close();
  }, [open, currency]);

  const updateRow = (index: number, patch: Partial<LimitRow>) =>
    setRows((prev) => prev.map((row, i) => (i === index ? { ...row, ...patch } : row)));

  const submit = async (event: Event) => {
    event.preventDefault();
    const trimmed = name.trim();
    const found: string[] = [];
    if (trimmed === '') found.push(t('agentKeys.problem_name'));
    if (trimmed.length > NAME_MAX) found.push(t('agentKeys.problem_name_long', { max: NAME_MAX }));
    if (rows.length === 0) found.push(t('agentKeys.problem_no_currency'));
    for (const row of rows) {
      const rowError = limitRowError(row);
      if (rowError) found.push(limitProblemText(rowError));
    }
    if (new Set(rows.map((row) => row.currency)).size !== rows.length) found.push(t('agentKeys.problem_duplicate'));
    setProblems(found);
    if (found.length > 0) return;

    setBusy(true);
    setError(null);
    try {
      const output = await keysCreate(client, {
        name: trimmed,
        tier,
        limits: limitsPayload(rows),
        ...(unitPin === '' ? {} : { unit: unitPin }),
      });
      onCreated(output);
    } catch (caught) {
      setError(caught);
    } finally {
      setBusy(false);
    }
  };

  return (
    <dialog
      ref={ref}
      class="confirm key-dialog"
      aria-labelledby="create-key-title"
      onClose={onClose}
      onCancel={(event) => {
        event.preventDefault();
        if (!busy) onClose();
      }}
    >
      <form class="dialog-body" onSubmit={submit} noValidate>
        <h2 id="create-key-title">{t('agentKeys.create_title')}</h2>
        <label class="field">
          <span>{t('agentKeys.name')}</span>
          <input
            class="input"
            name="name"
            aria-label={t('agentKeys.name')}
            value={name}
            maxLength={NAME_MAX + 20}
            autoComplete="off"
            aria-describedby="key-name-hint"
            onInput={(event) => setName((event.target as HTMLInputElement).value)}
          />
          <span id="key-name-hint" class="hint">
            {t('agentKeys.name_hint')}
          </span>
        </label>

        <fieldset class="field tiers">
          <legend>{t('agentKeys.tier_legend')}</legend>
          {TIERS.map((option) => (
            <label key={option.value} class="tier-option">
              <input
                type="radio"
                name="tier"
                value={option.value}
                checked={tier === option.value}
                onChange={() => setTier(option.value)}
              />
              <span>
                <strong>{t(option.labelKey)}</strong>
                <span class="hint"> {t(option.hintKey)}</span>
              </span>
            </label>
          ))}
        </fieldset>

        {units.length > 0 && (
          <div class="field unit-pin">
            <span id="key-unit-label">{t('units.pin_label')}</span>
            <Select
              id="key-unit"
              labelledBy="key-unit-label"
              value={unitPin}
              options={[
                { value: '', label: t('units.all_company') },
                ...units.map((unit) => ({ value: unit.code, label: unit.code, hint: unit.name, swatch: colourOf(unit) })),
              ]}
              searchable={false}
              onChange={setUnitPin}
            />
            <span class="hint">{t('units.pin_hint')}</span>
          </div>
        )}

        <fieldset class="field limits">
          <legend>{t('agentKeys.limits_legend')}</legend>
          <p class="hint">{t('agentKeys.limits_hint')}</p>
          {rows.map((row, index) => (
            <div class="limit-row" key={index}>
              <label class="field">
                <span>{t('agentKeys.currency')}</span>
                <input
                  class="input mono"
                  value={row.currency}
                  maxLength={3}
                  aria-label={t('agentKeys.currency_row', { n: index + 1 })}
                  onInput={(event) => updateRow(index, { currency: (event.target as HTMLInputElement).value.toUpperCase() })}
                />
              </label>
              <label class="field">
                <span>{t('agentKeys.per_entry')}</span>
                <input
                  class="input mono"
                  inputMode="decimal"
                  value={row.perEntry}
                  aria-label={t('agentKeys.per_entry_aria', { currency: row.currency || t('agentKeys.this_currency') })}
                  onInput={(event) => updateRow(index, { perEntry: (event.target as HTMLInputElement).value })}
                />
              </label>
              <label class="field">
                <span>{t('agentKeys.per_day')}</span>
                <input
                  class="input mono"
                  inputMode="decimal"
                  value={row.perDay}
                  aria-label={t('agentKeys.per_day_aria', { currency: row.currency || t('agentKeys.this_currency') })}
                  onInput={(event) => updateRow(index, { perDay: (event.target as HTMLInputElement).value })}
                />
              </label>
              <button
                type="button"
                class="btn btn-ghost btn-sm"
                disabled={rows.length === 1}
                onClick={() => setRows((prev) => prev.filter((_, i) => i !== index))}
              >
                {t('agentKeys.remove')}
                <span class="sr-only"> {t('agentKeys.remove_row', { n: index + 1 })}</span>
              </button>
            </div>
          ))}
          <div>
            <button
              type="button"
              class="btn btn-sm"
              onClick={() => setRows((prev) => [...prev, { currency: '', perEntry: '', perDay: '' }])}
            >
              {t('agentKeys.add_currency')}
            </button>
          </div>
        </fieldset>

        {problems.length > 0 && (
          <div class="banner error" role="alert">
            <ul class="problem-list">
              {problems.map((problem) => (
                <li key={problem}>{problem}</li>
              ))}
            </ul>
          </div>
        )}
        {error !== null && <ErrorBanner error={error} />}

        <div class="dialog-actions">
          <button type="button" class="btn" onClick={onClose} disabled={busy}>
            {t('common.cancel')}
          </button>
          <button type="submit" class="btn btn-primary" disabled={busy} aria-busy={busy ? 'true' : undefined}>
            {busy ? t('agentKeys.creating') : t('agentKeys.create')}
          </button>
        </div>
      </form>
    </dialog>
  );
}

/**
 * The show-once dialog: the full token, a Copy button, the warning that it is
 * never shown again, and the snippets with the real token in them. Closing it
 * drops the token from memory.
 */
export function ShowOnceDialog({
  output,
  baseUrl,
  onClose,
}: {
  output: KeysCreateOutput | null;
  baseUrl: string;
  onClose: () => void;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const tokenRef = useRef<HTMLInputElement>(null);
  const [copied, setCopied] = useState<'idle' | 'copied' | 'select'>('idle');

  useEffect(() => {
    const dialog = ref.current;
    if (!dialog) return;
    if (output && !dialog.open) dialog.showModal();
    if (!output && dialog.open) dialog.close();
  }, [output]);

  const copy = async () => {
    if (!output) return;
    try {
      await navigator.clipboard.writeText(output.token);
      setCopied('copied');
      return;
    } catch {
      // Clipboard API unavailable (insecure context, permissions): select the token for a manual copy.
    }
    const field = tokenRef.current;
    if (field) {
      field.focus();
      field.select();
      setCopied(document.execCommand?.('copy') ? 'copied' : 'select');
    }
  };

  return (
    <dialog
      ref={ref}
      class="confirm key-dialog"
      aria-labelledby="show-key-title"
      onClose={() => {
        setCopied('idle');
        onClose();
      }}
      onCancel={(event) => {
        // Esc still closes it, but the token is shown once: the person has to acknowledge it.
        event.preventDefault();
      }}
    >
      {output && (
        <div class="dialog-body">
          <h2 id="show-key-title">{t('agentKeys.copy_title')}</h2>
          <div class="banner human" role="alert">
            <div class="grow">
              <strong>{t('agentKeys.wont_see')}</strong> {t('agentKeys.wont_see_body')}
            </div>
          </div>
          <label class="field">
            <span>{output.key.name}</span>
            <div class="secret-row">
              <input
                ref={tokenRef}
                class="input mono secret"
                readOnly
                value={output.token}
                aria-describedby="show-key-note"
                onFocus={(event) => (event.target as HTMLInputElement).select()}
              />
              <button type="button" class="btn" onClick={() => void copy()}>
                {copied === 'copied' ? t('agentKeys.copied') : t('agentKeys.copy_key')}
              </button>
            </div>
            <span id="show-key-note" class="hint" role="status">
              {copied === 'select' ? t('agentKeys.copy_blocked') : t('agentKeys.prefix', { prefix: output.key.prefix ?? '' })}
            </span>
          </label>
          <UsageSnippets baseUrl={baseUrl} token={output.token} />
          <div class="dialog-actions">
            <button type="button" class="btn btn-primary" onClick={() => ref.current?.close()}>
              {t('agentKeys.copied_ack')}
            </button>
          </div>
        </div>
      )}
    </dialog>
  );
}
