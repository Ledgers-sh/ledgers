/**
 * Import parties from a CSV file. The file is checked first: the server reads
 * every row and says what each would do, and the import button stays off until
 * every row is valid. A file with any error is refused whole, so a dry run and
 * the write always agree.
 */
import { useState } from 'preact/hooks';
import { partyImport, partyImportCheck, type PartyImportCheck, type PartyImportResult } from '../api/parties';
import { t, type MessageKey } from '../i18n';
import { useSession } from '../session';
import { PageHead } from './common';
import { FormProblem } from './settings/common';
import { partyProblemKey, slugMessageKey } from '../lib/problems';

export function PartyImport() {
  const { client, toast } = useSession();
  const [csv, setCsv] = useState<string | null>(null);
  const [fileName, setFileName] = useState('');
  const [check, setCheck] = useState<PartyImportCheck | null>(null);
  const [checking, setChecking] = useState(false);
  const [importing, setImporting] = useState(false);
  const [done, setDone] = useState<PartyImportResult | null>(null);
  const [problem, setProblem] = useState<unknown>(null);
  const [problemKey, setProblemKey] = useState<MessageKey | null>(null);

  /** Shows a refusal in the app's words when it has them, the server's otherwise. */
  const refuse = (error: unknown) => {
    const key = partyProblemKey(error);
    setProblemKey(key);
    setProblem(key ? null : error);
  };

  const pick = async (file: File | undefined) => {
    setCheck(null);
    setDone(null);
    setProblem(null);
    setProblemKey(null);
    if (!file) {
      setCsv(null);
      setFileName('');
      return;
    }
    setCsv(await file.text());
    setFileName(file.name);
  };

  const runCheck = async () => {
    if (csv === null) {
      setProblemKey(null);
      toast(t('parties.import_pick_first'), 'error');
      return;
    }
    setChecking(true);
    setDone(null);
    setProblem(null);
    setProblemKey(null);
    try {
      setCheck(await partyImportCheck(client, csv));
    } catch (error) {
      setCheck(null);
      refuse(error);
    } finally {
      setChecking(false);
    }
  };

  const runImport = async () => {
    if (csv === null || !check?.valid) return;
    setImporting(true);
    setProblem(null);
    setProblemKey(null);
    try {
      const result = await partyImport(client, csv);
      setDone(result);
      setCheck(null);
      toast(t('parties.import_done', { created: result.created, updated: result.updated }), 'ok');
    } catch (error) {
      refuse(error);
    } finally {
      setImporting(false);
    }
  };

  return (
    <>
      <PageHead title={t('parties.import_title')}>{t('parties.import_intro')}</PageHead>
      <nav class="back-link-row">
        <a class="back-link" href="#/parties">
          {t('parties.back')}
        </a>
      </nav>

      <section class="panel party-section" aria-labelledby="import-file-title">
        <h2 id="import-file-title">{t('parties.import_choose')}</h2>
        <p class="muted">{t('parties.import_bank_note')}</p>
        <div class="party-import-controls">
          <input
            id="party-import-file"
            type="file"
            accept=".csv,text/csv"
            aria-label={t('parties.import_choose')}
            disabled={checking || importing}
            onChange={(event) => void pick((event.target as HTMLInputElement).files?.[0])}
          />
          {fileName && <span class="muted">{fileName}</span>}
          <button type="button" class="btn" onClick={() => void runCheck()} disabled={csv === null || checking || importing}>
            {checking ? t('parties.import_checking') : t('parties.import_check')}
          </button>
        </div>
        {problemKey && (
          <p class="field-error" role="alert">
            {t(problemKey)}
          </p>
        )}
        <FormProblem error={problem} />
      </section>

      {check && (
        <section class="panel party-section" aria-labelledby="import-preview-title">
          <h2 id="import-preview-title">{t('parties.import_title')}</h2>
          <p role="status" aria-live="polite">
            {t('parties.import_summary', { creates: check.creates, updates: check.updates, errors: check.errors })}{' '}
            <span class="muted">{check.valid ? t('parties.import_all_valid') : t('parties.import_has_errors')}</span>
          </p>
          <div class="table-wrap">
            <table class="ledger cards party-import-table">
              <caption class="sr-only">{t('parties.import_title')}</caption>
              <thead>
                <tr>
                  <th scope="col">{t('parties.col_line')}</th>
                  <th scope="col">{t('parties.col_name')}</th>
                  <th scope="col">{t('parties.col_result')}</th>
                </tr>
              </thead>
              <tbody>
                {check.rows.map((row) => (
                  <tr key={row.line} class={row.errors.length > 0 ? 'party-import-error' : undefined}>
                    <td class="main" data-label={t('parties.col_line')}>
                      {t('parties.import_line', { line: row.line })}
                    </td>
                    <td data-label={t('parties.col_name')}>
                      <span class="mono">{row.id}</span>{' '}
                      <span class="badge">
                        {row.action === 'create'
                          ? t('parties.import_action_create')
                          : row.action === 'update'
                            ? t('parties.import_action_update')
                            : t('parties.import_action_error')}
                      </span>
                    </td>
                    <td data-label={t('parties.col_result')}>
                      {row.errors.length === 0 ? (
                        <span class="muted">{t('parties.import_valid')}</span>
                      ) : (
                        <ul class="party-plain-list">
                          {row.errors.map((error, index) => {
                            const key = slugMessageKey(error.slug);
                            return (
                              <li key={`${row.line}-${index}`} class="field-error">
                                {key ? t(key) : error.message}
                              </li>
                            );
                          })}
                        </ul>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <button type="button" class="btn btn-primary" onClick={() => void runImport()} disabled={!check.valid || importing}>
            {t('parties.import_run', { n: check.rows.length })}
          </button>
        </section>
      )}

      {done && (
        <p class="banner" role="status">
          {t('parties.import_done', { created: done.created, updated: done.updated })}{' '}
          <a href="#/parties">{t('parties.back')}</a>
        </p>
      )}
    </>
  );
}
