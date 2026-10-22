/**
 * The statement import dialog. A file is dropped or picked; its format is read
 * from the bytes (camt.053, MT940, else CSV) and can be changed. A CSV needs a
 * saved column mapping first (the wizard, which saves it to the account), then
 * the file is stored and the server reads it into the account's lines. The
 * summary says what was new, what was already there, and whether the balances
 * were checked (a CSV has none, so it is never checked).
 */
import { useEffect, useRef, useState } from 'preact/hooks';
import {
  bankAccountUpdate,
  bankStatementsImport,
  type AccountView,
  type CsvMapping,
  type StatementsImportOutput,
} from '../api/bank';
import { uploadDocument } from '../api/bills';
import { useSession } from '../session';
import { detectFormat, formatIban } from '../lib/bank';
import { MAX_UPLOAD_BYTES } from '../lib/documents';
import { formatDate, t, type MessageKey } from '../i18n';
import { ErrorBanner, Money } from './common';
import { CsvWizard } from './BankCsvWizard';
import { Icon } from './icons';
import { Select } from './Select';

type Format = 'camt053' | 'mt940' | 'csv';

const FORMAT_KEYS: Record<Format, MessageKey> = {
  camt053: 'bank.format_camt053',
  mt940: 'bank.format_mt940',
  csv: 'bank.format_csv',
};

export function BankImport({ account, open, onClose, onImported }: { account: AccountView; open: boolean; onClose: () => void; onImported: () => void }) {
  const { client, toast } = useSession();
  const ref = useRef<HTMLDialogElement>(null);
  const fileInput = useRef<HTMLInputElement>(null);
  const [file, setFile] = useState<File | null>(null);
  const [text, setText] = useState<string>('');
  const [format, setFormat] = useState<Format>('camt053');
  const [mapping, setMapping] = useState<CsvMapping | null>(account.csv_mapping);
  const [editingMapping, setEditingMapping] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const [fileProblem, setFileProblem] = useState<MessageKey | null>(null);
  const [dragging, setDragging] = useState(false);
  const [summary, setSummary] = useState<StatementsImportOutput | null>(null);

  useEffect(() => {
    const dialog = ref.current;
    if (!dialog) return;
    if (open && !dialog.open) dialog.showModal();
    if (!open && dialog.open) dialog.close();
  }, [open]);

  // Each opening starts clean; the saved mapping comes from the account.
  useEffect(() => {
    if (!open) return;
    setFile(null);
    setText('');
    setFormat('camt053');
    setMapping(account.csv_mapping);
    setEditingMapping(false);
    setError(null);
    setFileProblem(null);
    setSummary(null);
  }, [open, account.id]);

  const readFile = async (picked: File) => {
    setFileProblem(null);
    setError(null);
    setSummary(null);
    if (picked.size > MAX_UPLOAD_BYTES) {
      setFileProblem('bank.import_too_large');
      return;
    }
    const content = await picked.text();
    setFile(picked);
    setText(content);
    setFormat(detectFormat(content));
    setEditingMapping(false);
  };

  const onPicked = (event: Event) => {
    const input = event.target as HTMLInputElement;
    const picked = input.files?.[0];
    input.value = '';
    if (picked) void readFile(picked);
  };

  const onDrop = (event: DragEvent) => {
    event.preventDefault();
    setDragging(false);
    const dropped = event.dataTransfer?.files?.[0];
    if (dropped) void readFile(dropped);
  };

  const saveMapping = async (next: CsvMapping) => {
    setError(null);
    await bankAccountUpdate(client, account.id, { csv_mapping: next });
    setMapping(next);
    setEditingMapping(false);
    toast(t('bank.toast_mapping_saved'));
  };

  const csvReady = format !== 'csv' || (mapping !== null && !editingMapping);
  const canImport = file !== null && csvReady && !busy && summary === null;

  const runImport = async () => {
    if (!file || !canImport) return;
    setBusy(true);
    setError(null);
    try {
      const stored = await uploadDocument(client, file, file.name);
      const result = await bankStatementsImport(client, { account_id: account.id, document_id: stored.document.id, format });
      setSummary(result);
      onImported();
    } catch (err) {
      setError(err);
    } finally {
      setBusy(false);
    }
  };

  const currency = account.currency;

  return (
    <dialog
      ref={ref}
      class="confirm bank-import"
      aria-labelledby="bank-import-title"
      onClose={onClose}
      onCancel={(event) => {
        event.preventDefault();
        if (!busy) onClose();
      }}
    >
      <div class="dialog-body">
        <h2 id="bank-import-title">{t('bank.import_title')}</h2>
        <p class="muted">
          <span class="mono">{formatIban(account.iban)}</span>
        </p>

        {summary ? (
          <section class="bank-summary" aria-labelledby="bank-summary-title" role="status">
            <h3 id="bank-summary-title" class="caption">
              {t('bank.summary_title')}
            </h3>
            <p>
              {t('bank.summary_totals', { added: summary.new_transactions, duplicate: summary.duplicate_transactions })}
            </p>
            {summary.statements.map((statement) => (
              <div class="bank-summary-statement" key={statement.ordinal}>
              <dl class="bank-summary-facts">
                <div>
                  <dt>{t('bank.summary_format')}</dt>
                  <dd>{t(FORMAT_KEYS[statement.format])}</dd>
                </div>
                {statement.reference && (
                  <div>
                    <dt>{t('bank.summary_reference')}</dt>
                    <dd class="mono">{statement.reference}</dd>
                  </div>
                )}
                <div>
                  <dt>{t('bank.summary_lines')}</dt>
                  <dd>
                    {t('bank.summary_lines_value', { lines: statement.lines, added: statement.new_transactions, duplicate: statement.duplicate_transactions })}
                  </dd>
                </div>
                <div>
                  <dt>{t('bank.summary_checked')}</dt>
                  <dd>{statement.balance_checked ? t('bank.summary_checked_yes') : t('bank.summary_checked_no')}</dd>
                </div>
                <div>
                  <dt>{t('bank.summary_opening')}</dt>
                  <dd>
                    {statement.opening ? (
                      <>
                        <span class="mono">{formatDate(statement.opening.date)}</span> <Money minor={String(statement.opening.minor)} currency={currency} />
                      </>
                    ) : (
                      t('bank.summary_none')
                    )}
                  </dd>
                </div>
                <div>
                  <dt>{t('bank.summary_closing')}</dt>
                  <dd>
                    {statement.closing ? (
                      <>
                        <span class="mono">{formatDate(statement.closing.date)}</span> <Money minor={String(statement.closing.minor)} currency={currency} />
                      </>
                    ) : (
                      t('bank.summary_none')
                    )}
                  </dd>
                </div>
              </dl>
                {statement.already_imported && <p class="note">{t('bank.summary_already')}</p>}
              </div>
            ))}
          </section>
        ) : (
          <>
            <div class={`dropzone${dragging ? ' dropzone-active' : ''}`} onDragOver={(event) => { event.preventDefault(); setDragging(true); }} onDragLeave={() => setDragging(false)} onDrop={onDrop}>
              <div class="dropzone-body">
                <strong>{file ? file.name : t('bank.import_drop')}</strong>
                <span class="muted">{file ? t('bank.import_loaded') : t('bank.import_formats')}</span>
                <button type="button" class="btn btn-sm" onClick={() => fileInput.current?.click()} disabled={busy}>
                  {file ? t('bank.import_other_file') : t('bank.import_choose')}
                </button>
                <input ref={fileInput} type="file" hidden accept=".xml,.sta,.940,.mt940,.txt,.csv,.tsv,text/csv,text/plain,application/xml" onChange={onPicked} />
              </div>
            </div>
            {fileProblem && (
              <p class="field-error" role="alert">
                {t(fileProblem)}
              </p>
            )}

            {file && (
              <div class="field">
                <span id="bank-import-format-label">{t('bank.import_format')}</span>
                <Select
                  id="bank-import-format"
                  labelledBy="bank-import-format-label"
                  value={format}
                  searchable={false}
                  onChange={(value) => {
                    setFormat(value as Format);
                    setEditingMapping(false);
                  }}
                  options={(['camt053', 'mt940', 'csv'] as const).map((value) => ({ value, label: t(FORMAT_KEYS[value]) }))}
                />
                <span class="hint">{t('bank.import_format_hint')}</span>
              </div>
            )}

            {file && format === 'csv' && (
              <section class="bank-csv-mapping" aria-labelledby="bank-csv-title">
                <h3 id="bank-csv-title" class="caption">
                  {t('bank.csv_wizard_title')}
                </h3>
                {mapping && !editingMapping ? (
                  <div class="row-actions">
                    <p class="note">{t('bank.csv_mapping_saved')}</p>
                    <button type="button" class="btn btn-sm" onClick={() => setEditingMapping(true)}>
                      {t('bank.csv_change_mapping')}
                    </button>
                  </div>
                ) : (
                  <CsvWizard
                    idPrefix="bank-import-csv"
                    sample={text}
                    currency={currency}
                    initial={mapping}
                    saveLabel={t('bank.csv_save_mapping')}
                    onSave={saveMapping}
                  />
                )}
              </section>
            )}

            {account.archived && <p class="note">{t('bank.import_archived')}</p>}
          </>
        )}

        {error !== null && <ErrorBanner error={error} />}

        <div class="dialog-actions">
          {summary ? (
            <button type="button" class="btn btn-primary" onClick={onClose}>
              {t('bank.summary_done')}
            </button>
          ) : (
            <>
              <button type="button" class="btn" onClick={onClose} disabled={busy}>
                {t('common.cancel')}
              </button>
              <button type="button" class="btn btn-primary" onClick={() => void runImport()} disabled={!canImport} aria-busy={busy ? 'true' : undefined}>
                <Icon name="check" />
                {busy ? t('common.working') : t('bank.import_submit')}
              </button>
            </>
          )}
        </div>
      </div>
    </dialog>
  );
}
