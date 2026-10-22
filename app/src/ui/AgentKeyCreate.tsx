import { useEffect, useRef, useState } from 'preact/hooks';
import { keysCreate } from '../api/commands';
import type { KeysCreateOutput } from '../api/types';
import { limitRowError, limitsPayload, TIERS, type LimitRow, type TierValue } from '../lib/agentKeys';
import { useSession } from '../session';
import { ErrorBanner } from './common';
import { UsageSnippets } from './UsageSnippets';

const NAME_MAX = 80;

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
  const { client, currency } = useSession();
  const ref = useRef<HTMLDialogElement>(null);
  const [name, setName] = useState('');
  const [tier, setTier] = useState<TierValue>('post-within-limits');
  const [rows, setRows] = useState<LimitRow[]>(() => [{ currency, perEntry: '', perDay: '' }]);
  const [problems, setProblems] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);

  useEffect(() => {
    const dialog = ref.current;
    if (!dialog) return;
    if (open && !dialog.open) {
      setName('');
      setTier('post-within-limits');
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
    if (trimmed === '') found.push('Give the key a name, such as "Bank feed bot".');
    if (trimmed.length > NAME_MAX) found.push(`The name can be ${NAME_MAX} characters at most.`);
    if (rows.length === 0) found.push('Add at least one currency. A key with no currency cannot move money.');
    for (const row of rows) {
      const rowError = limitRowError(row);
      if (rowError) found.push(rowError);
    }
    if (new Set(rows.map((row) => row.currency)).size !== rows.length) found.push('Each currency can appear once.');
    setProblems(found);
    if (found.length > 0) return;

    setBusy(true);
    setError(null);
    try {
      const output = await keysCreate(client, { name: trimmed, tier, limits: limitsPayload(rows) });
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
        <h2 id="create-key-title">Create an agent key</h2>
        <label class="field">
          <span>Name</span>
          <input
            class="input"
            name="name"
            aria-label="Name"
            value={name}
            maxLength={NAME_MAX + 20}
            autoComplete="off"
            aria-describedby="key-name-hint"
            onInput={(event) => setName((event.target as HTMLInputElement).value)}
          />
          <span id="key-name-hint" class="hint">Shown in the list and the audit trail. Not secret.</span>
        </label>

        <fieldset class="field tiers">
          <legend>Tier</legend>
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
                <strong>{option.label}</strong>
                <span class="hint"> {option.hint}</span>
              </span>
            </label>
          ))}
        </fieldset>

        <fieldset class="field limits">
          <legend>Limits per currency</legend>
          <p class="hint">
            The most one entry may move, and the most in a rolling day, in major units (for example 1000.00). A currency
            with no row cannot be moved at all. Read and draft keys do not post, so their limits do nothing yet.
          </p>
          {rows.map((row, index) => (
            <div class="limit-row" key={index}>
              <label class="field">
                <span>Currency</span>
                <input
                  class="input mono"
                  value={row.currency}
                  maxLength={3}
                  aria-label={`Currency, row ${index + 1}`}
                  onInput={(event) => updateRow(index, { currency: (event.target as HTMLInputElement).value.toUpperCase() })}
                />
              </label>
              <label class="field">
                <span>Per entry</span>
                <input
                  class="input mono"
                  inputMode="decimal"
                  value={row.perEntry}
                  aria-label={`Most per entry in ${row.currency || 'this currency'}`}
                  onInput={(event) => updateRow(index, { perEntry: (event.target as HTMLInputElement).value })}
                />
              </label>
              <label class="field">
                <span>Per day</span>
                <input
                  class="input mono"
                  inputMode="decimal"
                  value={row.perDay}
                  aria-label={`Most per day in ${row.currency || 'this currency'}`}
                  onInput={(event) => updateRow(index, { perDay: (event.target as HTMLInputElement).value })}
                />
              </label>
              <button
                type="button"
                class="btn btn-ghost btn-sm"
                disabled={rows.length === 1}
                onClick={() => setRows((prev) => prev.filter((_, i) => i !== index))}
              >
                Remove<span class="sr-only"> currency row {index + 1}</span>
              </button>
            </div>
          ))}
          <div>
            <button
              type="button"
              class="btn btn-sm"
              onClick={() => setRows((prev) => [...prev, { currency: '', perEntry: '', perDay: '' }])}
            >
              Add currency
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
            Cancel
          </button>
          <button type="submit" class="btn btn-primary" disabled={busy} aria-busy={busy ? 'true' : undefined}>
            {busy ? 'Creating…' : 'Create key'}
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
          <h2 id="show-key-title">Copy your new key</h2>
          <div class="banner human" role="alert">
            <div class="grow">
              <strong>You won't see this again.</strong> Copy it now and store it where the agent can read it. If you lose it,
              revoke this key and create a new one.
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
                {copied === 'copied' ? 'Copied' : 'Copy key'}
              </button>
            </div>
            <span id="show-key-note" class="hint" role="status">
              {copied === 'select' ? 'Copying is blocked here. The key is selected: press Ctrl+C or Cmd+C.' : `Prefix ${output.key.prefix ?? ''}`}
            </span>
          </label>
          <UsageSnippets baseUrl={baseUrl} token={output.token} />
          <div class="dialog-actions">
            <button type="button" class="btn btn-primary" onClick={() => ref.current?.close()}>
              I've copied the key
            </button>
          </div>
        </div>
      )}
    </dialog>
  );
}
