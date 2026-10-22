import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { t } from '../i18n';
import { accessFor, filterCommands, type PaletteCommand, type Session } from './model';

/** The palette's data: the commands, or why there are none yet. */
export type PaletteData = { status: 'loading' } | { status: 'unavailable' } | { status: 'ready'; commands: PaletteCommand[] };

/** Copies text to the clipboard. Resolves false when the browser refuses (no permission, insecure page). */
export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

/**
 * ⌘K / Ctrl+K command palette. Lists every operation of the server's OpenAPI
 * document with its `ledgers …` CLI line, MCP tool name and HTTP call, each
 * with a copy button. A command the session may run in this app opens its
 * screen; the rest say why they are not offered here.
 */
export function CommandPalette({
  open,
  onClose,
  data,
  session,
  onOpenHash,
}: {
  open: boolean;
  onClose: () => void;
  data: PaletteData;
  session: Session;
  onOpenHash: (hash: string) => void;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const [query, setQuery] = useState('');
  const [status, setStatus] = useState('');

  useEffect(() => {
    const dialog = ref.current;
    if (!dialog) return;
    if (open && !dialog.open) {
      dialog.showModal();
      setQuery('');
      setStatus('');
      window.setTimeout(() => searchRef.current?.focus(), 0);
    }
    if (!open && dialog.open) dialog.close();
  }, [open]);

  const commands = data.status === 'ready' ? data.commands : [];
  const matches = useMemo(() => filterCommands(commands, query), [commands, query]);

  const copy = async (label: string, value: string) => {
    const ok = await copyText(value);
    setStatus(ok ? t('palette.copied', { what: label }) : t('palette.copy_failed'));
  };

  return (
    <dialog
      ref={ref}
      class="palette"
      aria-labelledby="palette-title"
      onClose={onClose}
      onCancel={(event) => {
        event.preventDefault();
        onClose();
      }}
    >
      <div class="palette-body">
        <div class="palette-head">
          <h2 id="palette-title" class="grow">
            {t('palette.dialog_label')}
          </h2>
          <button type="button" class="btn btn-ghost btn-sm" onClick={onClose} aria-label={t('palette.close')}>
            ×
          </button>
        </div>

        <label class="sr-only" for="palette-search">
          {t('palette.search_label')}
        </label>
        <input
          id="palette-search"
          ref={searchRef}
          class="input palette-search"
          type="search"
          autoComplete="off"
          spellcheck={false}
          placeholder={t('palette.search_placeholder')}
          value={query}
          disabled={data.status !== 'ready'}
          onInput={(event) => setQuery((event.target as HTMLInputElement).value)}
        />

        {session.kind === 'key' && <p class="muted palette-note">{t('palette.key_note')}</p>}

        <div class="sr-only" role="status" aria-live="polite">
          {status}
        </div>

        {data.status === 'loading' && <p class="muted" role="status">{t('palette.loading')}</p>}
        {data.status === 'unavailable' && (
          <p class="result fail" role="alert">
            <span>{t('palette.unavailable')}</span>
          </p>
        )}
        {data.status === 'ready' && (
          <>
            <p class="muted palette-count">{t('palette.shown', { shown: matches.length, total: commands.length })}</p>
            {matches.length === 0 ? (
              <p class="muted">{t('palette.none', { query })}</p>
            ) : (
              <ul class="palette-list" aria-label={t('palette.dialog_label')}>
                {matches.map((command) => (
                  <CommandRow key={command.operationId} command={command} session={session} onCopy={copy} onOpenHash={onOpenHash} />
                ))}
              </ul>
            )}
          </>
        )}
      </div>
    </dialog>
  );
}

function CommandRow({
  command,
  session,
  onCopy,
  onOpenHash,
}: {
  command: PaletteCommand;
  session: Session;
  onCopy: (label: string, value: string) => void;
  onOpenHash: (hash: string) => void;
}) {
  const access = accessFor(command, session);
  const forms: { label: string; value: string | null; missing: string; copy: string | null }[] = [
    { label: t('palette.cli'), value: command.cli, missing: t('common.empty_cell'), copy: command.cli },
    { label: t('palette.mcp'), value: command.mcp, missing: t('palette.no_mcp'), copy: command.mcp },
    { label: t('palette.http'), value: `${command.method} ${command.path}`, missing: t('common.empty_cell'), copy: command.curl },
  ];

  let reason: string | null = null;
  if (!access.runnable) {
    if (access.reason === 'human-only') reason = t('palette.human_only');
    else if (access.reason === 'role') reason = t('palette.role_denied');
    else reason = t('palette.no_screen');
  }

  return (
    <li class="palette-item">
      <div class="palette-item-head">
        <h3 class="palette-name">
          <code>{command.command}</code>
        </h3>
        <span class="badge">{t('palette.tier', { tier: command.tier })}</span>
        {command.humanOnly && <span class="badge human">{t('marker.person')}</span>}
      </div>
      <div class="palette-forms">
        {forms.map((form) => (
          <div class="palette-form" key={form.label}>
            <span class="palette-label">{form.label}</span>
            <div class="palette-cell">
              {form.value === null ? <span class="muted">{form.missing}</span> : <code class="palette-value">{form.value}</code>}
            </div>
            {form.copy !== null && (
              <button
                type="button"
                class="btn btn-ghost btn-sm"
                aria-label={`${t('palette.copy_what', { what: form.label })}: ${command.command}`}
                onClick={() => onCopy(form.label, form.copy as string)}
              >
                {t('palette.copy')}
              </button>
            )}
          </div>
        ))}
      </div>
      <div class="palette-action">
        {access.runnable ? (
          <button type="button" class="btn btn-sm btn-primary" onClick={() => onOpenHash(access.hash)}>
            {t('palette.open_screen', { screen: t(access.label) })}
          </button>
        ) : (
          <p class="muted palette-reason">{reason}</p>
        )}
      </div>
    </li>
  );
}
