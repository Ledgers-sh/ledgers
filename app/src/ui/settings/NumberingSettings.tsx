import { useState } from 'preact/hooks';
import { numberingList, numberingSetNext, type JournalNumberingView, type SequenceView } from '../../api/settings';
import { useAsync } from '../../hooks';
import { t } from '../../i18n';
import { useSession } from '../../session';
import { describeError, Empty, ErrorBanner } from '../common';
import { FieldProblem, LoadingLine, SettingsPanel } from './common';

/**
 * A typed next number: whole digits, and above the number that is next now
 * (numbers only move forward). Returns the problem, or null when it is fine.
 */
export function nextNumberProblem(raw: string, nextNow: number): 'whole' | 'forward' | null {
  const value = raw.trim();
  if (!/^\d+$/.test(value)) return 'whole';
  return Number(value) > nextNow ? null : 'forward';
}

/** Settings → Numbering: each journal's next entry number per fiscal year. Numbers only move forward, and only before one is used. */
export function NumberingSettings() {
  const { client, toast } = useSession();
  const state = useAsync(() => numberingList(client), [client.baseUrl, client.apiKey]);
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [problems, setProblems] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<string | null>(null);

  if (state.error && !state.data) return <ErrorBanner error={state.error} onRetry={state.reload} />;
  if (!state.data) return <LoadingLine />;
  const journals = state.data.journals;

  const keyOf = (journal: JournalNumberingView, sequence: SequenceView) => `${journal.id}/${sequence.fiscal_year}`;

  async function save(journal: JournalNumberingView, sequence: SequenceView) {
    const key = keyOf(journal, sequence);
    const raw = drafts[key] ?? String(sequence.next_number);
    const problem = nextNumberProblem(raw, sequence.next_number);
    if (problem) {
      setProblems({ ...problems, [key]: problem === 'whole' ? t('settings.number_whole') : t('settings.number_forward', { next: sequence.next_number }) });
      document.getElementById(`number-${key.replace(/[^a-zA-Z0-9]/g, '-')}`)?.focus();
      return;
    }
    setBusy(key);
    try {
      await numberingSetNext(client, { journal_id: journal.id, fiscal_year: sequence.fiscal_year, next_number: Number(raw) });
      setProblems({ ...problems, [key]: '' });
      setDrafts(({ [key]: _saved, ...rest }) => rest);
      toast(t('settings.number_saved', { code: journal.code, year: sequence.fiscal_year }));
      state.reload();
    } catch (err) {
      const described = describeError(err);
      setProblems({ ...problems, [key]: described.detail ?? described.title });
    } finally {
      setBusy(null);
    }
  }

  if (journals.length === 0) {
    return <Empty title={t('settings.numbering_empty_title')}>{t('settings.numbering_empty_body')}</Empty>;
  }

  return (
    <>
      {journals.map((journal) => (
        <SettingsPanel key={journal.id} id={`journal-${journal.id}-title`} title={`${journal.code} · ${journal.name}`}>
          <div class="table-wrap">
            <table class="settings-table">
              <thead>
                <tr>
                  <th scope="col">{t('settings.col_fiscal_year')}</th>
                  <th scope="col" class="num">
                    {t('settings.col_entries')}
                  </th>
                  <th scope="col">{t('settings.col_next_number')}</th>
                </tr>
              </thead>
              <tbody>
                {journal.sequences.map((sequence) => {
                  const key = keyOf(journal, sequence);
                  const inputId = `number-${key.replace(/[^a-zA-Z0-9]/g, '-')}`;
                  const message = problems[key] || null;
                  return (
                    <tr key={key}>
                      <th scope="row">{sequence.fiscal_year}</th>
                      <td class="num">{sequence.entries}</td>
                      <td>
                        {sequence.editable ? (
                          <form
                            class="inline-form"
                            noValidate
                            onSubmit={(event) => {
                              event.preventDefault();
                              void save(journal, sequence);
                            }}
                          >
                            <label class="sr-only" for={inputId}>
                              {t('settings.next_number_named', { code: journal.code, year: sequence.fiscal_year })}
                            </label>
                            <input
                              id={inputId}
                              class="input num"
                              type="number"
                              inputMode="numeric"
                              min={sequence.next_number + 1}
                              value={drafts[key] ?? String(sequence.next_number)}
                              aria-invalid={message ? 'true' : undefined}
                              aria-describedby={message ? `${inputId}-error` : `${inputId}-hint`}
                              onInput={(event) => setDrafts({ ...drafts, [key]: (event.target as HTMLInputElement).value })}
                            />
                            <button type="submit" class="btn btn-sm" disabled={busy !== null}>
                              {t('settings.save')}
                            </button>
                            <span id={`${inputId}-hint`} class="hint">
                              {t('settings.number_hint')}
                            </span>
                            <FieldProblem id={`${inputId}-error`} message={message} />
                          </form>
                        ) : (
                          <span>
                            <span class="mono">{sequence.next_number}</span>{' '}
                            <span class="hint">{t('settings.number_locked', { count: sequence.entries })}</span>
                          </span>
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </SettingsPanel>
      ))}
    </>
  );
}
