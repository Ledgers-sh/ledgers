/**
 * Business units: the list, add, rename (name, colour, logo) and archive.
 * Both the Accounts area and Settings show this panel. Writes are a person's:
 * an agent key sees the list and the transfer, and an explanation in place of
 * the forms. An archived unit takes no changes and no new postings.
 */
import { useEffect, useRef, useState } from 'preact/hooks';
import { unitsArchive, unitsCreate, unitsList, unitsRename } from '../api/commands';
import { uploadDocument } from '../api/bills';
import { ApiError } from '../api/client';
import type { Unit } from '../api/types';
import { useAsync } from '../hooks';
import { useSession } from '../session';
import { t, type MessageKey } from '../i18n';
import { Ltr } from '../i18n/Ltr';
import { logoProblem } from '../lib/logos';
import { Avatar } from './Avatar';
import { colourOf, UNIT_CODE, UNIT_COLOUR } from '../lib/units';
import { ConfirmDialog, Empty, ErrorBanner, PageHead, Skeleton } from './common';
import { Icon } from './icons';
import { TransferDialog } from './TransferDialog';

/** The business units page under Accounts (#/accounts/units). */
export function Units() {
  return (
    <>
      <PageHead title={t('units.title')}>{t('units.intro')}</PageHead>
      <UnitsPanel />
    </>
  );
}

/** The list of units with the actions the signed-in person may take. Used by the page and by Settings. */
export function UnitsPanel() {
  const { client, refreshUnits, units: activeList } = useSession();
  const person = !client.apiKey;
  const list = useAsync(() => unitsList(client).then((r) => r.units), [client.baseUrl, client.apiKey, activeList.length]);
  const [editing, setEditing] = useState<Unit | 'new' | null>(null);
  const [archiving, setArchiving] = useState<Unit | null>(null);
  const [transferOpen, setTransferOpen] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  if (list.loading && !list.data) return <Skeleton rows={4} label={t('units.loading')} />;
  if (list.error && !list.data) return <ErrorBanner error={list.error} onRetry={list.reload} />;
  const all = [...(list.data ?? [])].sort((a, b) => a.code.localeCompare(b.code));
  const activeCount = all.filter((unit) => !unit.archived).length;

  return (
    <div class="units-panel">
      <div class="row-actions units-toolbar">
        {person && (
          <button type="button" class="btn btn-primary" onClick={() => setEditing('new')}>
            <Icon name="plus" />
            {t('units.add')}
          </button>
        )}
        <button type="button" class="btn" onClick={() => setTransferOpen(true)} disabled={activeCount < 2}>
          {t('units.transfer')}
        </button>
        {!person && <p class="note grow">{t('units.key_read_only')}</p>}
      </div>
      {message && (
        <div class="banner" role="status">
          {message}
        </div>
      )}
      {all.length === 0 ? (
        <Empty
          title={t('units.none_title')}
          action={
            person ? (
              <button type="button" class="btn btn-primary" onClick={() => setEditing('new')}>
                {t('units.add')}
              </button>
            ) : undefined
          }
        >
          {t('units.none_body')}
        </Empty>
      ) : (
        <div class="table-wrap">
          <table class="ledger cards units-table">
            <caption class="sr-only">{t('units.table_caption')}</caption>
            <thead>
              <tr>
                <th scope="col">{t('units.code')}</th>
                <th scope="col">{t('units.name')}</th>
                <th scope="col">{t('units.colour')}</th>
                <th scope="col">{t('units.status')}</th>
                <th scope="col">
                  <span class="sr-only">{t('units.actions')}</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {all.map((unit) => (
                <tr key={unit.code}>
                  <td data-label={t('units.code')} class="mono">
                    <Avatar name={unit.name} colour={colourOf(unit)} logo={unit.logo_document_id ? { documentId: unit.logo_document_id } : null} size="sm" />
                    <Ltr>{unit.code}</Ltr>
                  </td>
                  <td data-label={t('units.name')}>{unit.name}</td>
                  <td data-label={t('units.colour')}>
                    <span class="mono muted">
                      <Ltr>{unit.colour}</Ltr>
                    </span>
                  </td>
                  <td data-label={t('units.status')}>
                    <span class={`badge ${unit.archived ? '' : 'active'}`}>{unit.archived ? t('units.archived') : t('units.active')}</span>
                  </td>
                  <td class="actions">
                    {person && !unit.archived && (
                      <span class="row-actions">
                        <button type="button" class="btn btn-sm" onClick={() => setEditing(unit)}>
                          {t('units.edit')}
                          <span class="sr-only"> {unit.code}</span>
                        </button>
                        <button type="button" class="btn btn-sm btn-ghost" onClick={() => setArchiving(unit)}>
                          {t('units.archive')}
                          <span class="sr-only"> {unit.code}</span>
                        </button>
                      </span>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {editing !== null && (
        <UnitDialog
          key={editing === 'new' ? 'new' : editing.code}
          unit={editing === 'new' ? null : editing}
          onClose={() => setEditing(null)}
          onSaved={(saved) => {
            setEditing(null);
            setMessage(editing === 'new' ? t('units.created', { code: saved.code }) : t('units.saved'));
            list.reload();
            refreshUnits();
          }}
        />
      )}

      <ArchiveDialog
        unit={archiving}
        onClose={() => setArchiving(null)}
        onArchived={(code) => {
          setArchiving(null);
          setMessage(t('units.archived_toast', { code }));
          list.reload();
          refreshUnits();
        }}
      />

      <TransferDialog
        open={transferOpen}
        onClose={() => setTransferOpen(false)}
        units={activeList}
        onPosted={() => setMessage(null)}
      />
    </div>
  );
}

/** Create (unit is null) or rename a unit. Only the fields that changed are sent on rename. */
function UnitDialog({ unit, onClose, onSaved }: { unit: Unit | null; onClose: () => void; onSaved: (unit: Unit) => void }) {
  const { client } = useSession();
  const ref = useRef<HTMLDialogElement>(null);
  const [code, setCode] = useState(unit?.code ?? '');
  const [name, setName] = useState(unit?.name ?? '');
  const [colour, setColour] = useState(colourOf(unit));
  const [logoId, setLogoId] = useState<string | null>(unit?.logo_document_id ?? null);
  const [logoName, setLogoName] = useState<string | null>(null);
  const [logoClear, setLogoClear] = useState(false);
  const [touched, setTouched] = useState(false);
  const [codeTaken, setCodeTaken] = useState(false);
  const [busy, setBusy] = useState(false);
  const [uploading, setUploading] = useState(false);
  const [logoError, setLogoError] = useState<MessageKey | null>(null);
  const [error, setError] = useState<unknown>(null);

  useEffect(() => {
    const dialog = ref.current;
    if (dialog && !dialog.open) dialog.showModal();
    return () => {
      if (dialog?.open) dialog.close();
    };
  }, []);

  const codeError: MessageKey | null = !unit && touched && !UNIT_CODE.test(code.trim()) ? 'units.err_code' : codeTaken ? 'units.err_taken' : null;
  const nameError: MessageKey | null = touched && (name.trim() === '' || name.trim().length > 80) ? 'units.err_name' : null;
  const colourError: MessageKey | null = touched && !UNIT_COLOUR.test(colour) ? 'units.err_colour' : null;

  const pickLogo = async (file: File | undefined) => {
    if (!file) return;
    setLogoError(null);
    const problem = logoProblem(file);
    if (problem) {
      setLogoError(problem === 'too-large' ? 'units.logo_too_large' : 'units.logo_unsupported');
      return;
    }
    setUploading(true);
    try {
      const stored = await uploadDocument(client, file, file.name);
      setLogoId(stored.document.id);
      setLogoName(file.name);
      setLogoClear(false);
    } catch (caught) {
      setLogoError(caught instanceof ApiError && caught.status === 413 ? 'units.logo_too_large' : 'units.logo_failed');
    } finally {
      setUploading(false);
    }
  };

  const submit = async (event: Event) => {
    event.preventDefault();
    setTouched(true);
    setError(null);
    setCodeTaken(false);
    const trimmedName = name.trim();
    const bad = (!unit && !UNIT_CODE.test(code.trim())) || trimmedName === '' || trimmedName.length > 80 || !UNIT_COLOUR.test(colour);
    if (bad || uploading) return;
    setBusy(true);
    try {
      if (unit) {
        const changes: Parameters<typeof unitsRename>[2] = { name: trimmedName, colour };
        if (logoClear) changes.clear_logo = true;
        else if (logoName !== null && logoId) changes.logo_document_id = logoId;
        const result = await unitsRename(client, unit.code, changes);
        onSaved(result.unit);
      } else {
        const result = await unitsCreate(client, {
          code: code.trim().toUpperCase(),
          name: trimmedName,
          colour,
          ...(logoId ? { logo_document_id: logoId } : {}),
        });
        onSaved(result.unit);
      }
    } catch (caught) {
      if (caught instanceof ApiError && caught.slug === 'unit-conflict') setCodeTaken(true);
      setError(caught);
    } finally {
      setBusy(false);
    }
  };

  return (
    <dialog
      ref={ref}
      class="confirm key-dialog unit-dialog"
      aria-labelledby="unit-dialog-title"
      onClose={onClose}
      onCancel={(event) => {
        event.preventDefault();
        if (!busy) onClose();
      }}
    >
      <form class="dialog-body" onSubmit={submit} noValidate>
        <h2 id="unit-dialog-title">{unit ? t('units.edit_title', { code: unit.code }) : t('units.add_title')}</h2>
        <label class="field">
          <span>{t('units.code')}</span>
          <input
            class="input mono"
            name="code"
            value={unit ? unit.code : code}
            readOnly={unit !== null}
            maxLength={16}
            autoComplete="off"
            aria-invalid={codeError ? 'true' : undefined}
            aria-describedby={codeError ? 'unit-code-error' : 'unit-code-hint'}
            onInput={(event) => setCode((event.target as HTMLInputElement).value.toUpperCase())}
          />
          {codeError ? (
            <span id="unit-code-error" class="field-error">{t(codeError)}</span>
          ) : (
            <span id="unit-code-hint" class="hint">{unit ? t('units.code_fixed') : t('units.code_hint')}</span>
          )}
        </label>
        <label class="field">
          <span>{t('units.name')}</span>
          <input
            class="input"
            name="name"
            maxLength={80}
            value={name}
            aria-invalid={nameError ? 'true' : undefined}
            aria-describedby={nameError ? 'unit-name-error' : undefined}
            onInput={(event) => setName((event.target as HTMLInputElement).value)}
          />
          {nameError && <span id="unit-name-error" class="field-error">{t(nameError)}</span>}
        </label>
        <div class="field">
          <label for="unit-colour">{t('units.colour')}</label>
          <div class="colour-field">
            <input
              id="unit-colour"
              class="colour-input"
              type="color"
              value={colour.toLowerCase()}
              aria-invalid={colourError ? 'true' : undefined}
              onInput={(event) => setColour((event.target as HTMLInputElement).value.toUpperCase())}
            />
            <span class="mono muted">
              <Ltr>{colour}</Ltr>
            </span>
          </div>
          {colourError && <span class="field-error">{t(colourError)}</span>}
        </div>
        <div class="field">
          <span id="unit-logo-label">{t('units.logo')}</span>
          <div class="logo-field">
            <input
              class="input"
              type="file"
              accept="image/png,image/jpeg"
              aria-labelledby="unit-logo-label"
              onChange={(event) => void pickLogo((event.target as HTMLInputElement).files?.[0])}
              disabled={uploading}
            />
            <span class="hint" role="status">
              {uploading
                ? t('units.logo_uploading')
                : logoClear
                  ? t('units.logo_removed')
                  : logoId
                    ? t('units.logo_set')
                    : t('units.logo_none')}
            </span>
            {logoId && !logoClear && (
              <button type="button" class="btn btn-ghost btn-sm" onClick={() => { setLogoId(null); setLogoName(null); setLogoClear(true); }}>
                {t('units.logo_remove')}
              </button>
            )}
          </div>
          {logoError && <span class="field-error">{t(logoError)}</span>}
        </div>
        {error !== null && <ErrorBanner error={error} />}
        <div class="dialog-actions">
          <button type="button" class="btn" onClick={onClose} disabled={busy}>
            {t('units.cancel')}
          </button>
          <button type="submit" class="btn btn-primary" disabled={busy || uploading} aria-busy={busy ? 'true' : undefined}>
            {busy ? t('common.working') : unit ? t('units.save') : t('units.add_submit')}
          </button>
        </div>
      </form>
    </dialog>
  );
}

function ArchiveDialog({ unit, onClose, onArchived }: { unit: Unit | null; onClose: () => void; onArchived: (code: string) => void }) {
  const { client } = useSession();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  useEffect(() => setError(null), [unit?.code]);
  if (!unit) return <ConfirmDialog open={false} onClose={onClose} title="" confirmLabel="" onConfirm={() => {}}>{null}</ConfirmDialog>;
  return (
    <ConfirmDialog
      open
      onClose={onClose}
      title={t('units.archive_title', { code: unit.code })}
      confirmLabel={t('units.archive_confirm')}
      danger
      busy={busy}
      error={error}
      onConfirm={() => {
        setBusy(true);
        setError(null);
        unitsArchive(client, unit.code).then(
          () => {
            setBusy(false);
            onArchived(unit.code);
          },
          (caught: unknown) => {
            setBusy(false);
            setError(caught);
          },
        );
      }}
    >
      <p class="muted">{t('units.archive_body', { code: unit.code })}</p>
    </ConfirmDialog>
  );
}
