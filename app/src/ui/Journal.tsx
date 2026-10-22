import { useMemo, useRef, useState } from 'preact/hooks';
import { entriesGet, entriesList, entriesReverse } from '../api/commands';
import type { EntryRecord } from '../api/types';
import { useAsync } from '../hooks';
import { useSession } from '../session';
import { todayIso } from '../lib/period';
import { ConfirmDialog, Empty, ErrorBanner, Marker, Money, PageHead, Skeleton } from './common';
import { formatMinor } from '../lib/money';
import { formatDate, intlLocale, t, tRich } from '../i18n';
import { Ltr } from '../i18n/Ltr';

/** Entry number as shown to people: fiscal year plus the gapless number. */
export function entryRef(entry: Pick<EntryRecord, 'fiscal_year' | 'number'>): string {
  return `FY${entry.fiscal_year}-${String(entry.number).padStart(4, '0')}`;
}

/** Sum of debit lines in an entry, per its base currency. */
export function entryTotal(entry: EntryRecord): { minor: string; currency: string } {
  let total = 0n;
  for (const line of entry.lines) {
    if (line.side === 'debit') total += BigInt(line.base.minor);
  }
  return { minor: total.toString(), currency: entry.base_currency };
}

export function Journal() {
  const { client } = useSession();
  const [query, setQuery] = useState('');
  const state = useAsync(async () => (await entriesList(client)).entries, [client.baseUrl, client.apiKey]);
  const searchRef = useRef<HTMLInputElement>(null);

  const entries = useMemo(() => {
    const sorted = [...(state.data ?? [])].sort(
      (a, b) => b.posting_date.localeCompare(a.posting_date) || b.number - a.number,
    );
    const needle = query.trim().toLowerCase();
    if (!needle) return sorted;
    return sorted.filter(
      (entry) =>
        entry.description.toLowerCase().includes(needle) ||
        entryRef(entry).toLowerCase().includes(needle) ||
        (entry.external_id ?? '').toLowerCase().includes(needle),
    );
  }, [state.data, query]);

  if (state.loading && !state.data) return <Skeleton rows={6} label={t('journal.loading')} />;
  if (state.error && !state.data) return <ErrorBanner error={state.error} onRetry={state.reload} />;
  const all = state.data ?? [];

  return (
    <>
      <PageHead
        title={t('journal.title')}
        actions={
          <a class="btn btn-primary" href="#/new">
            {t('shell.new_entry')}
          </a>
        }
      >
        {t('journal.intro')}
      </PageHead>

      {all.length === 0 ? (
        <Empty
          title={t('journal.empty_title')}
          action={
            <a class="btn btn-primary" href="#/new">
              {t('action.post_first')}
            </a>
          }
        >
          {t('journal.empty_body')}
        </Empty>
      ) : (
        <section class="panel" aria-label={t('journal.section')}>
          <div class="panel-head">
            <label class="sr-only" for="journal-search">
              {t('journal.search_label')}
            </label>
            <input
              id="journal-search"
              ref={searchRef}
              class="input"
              type="search"
              placeholder={t('journal.search_placeholder')}
              value={query}
              onInput={(event) => setQuery((event.target as HTMLInputElement).value)}
              style={{ maxWidth: '360px' }}
            />
            <span class="grow" />
            <span class="muted">{t('journal.count', { shown: entries.length, total: all.length })}</span>
          </div>
          {entries.length === 0 ? (
            <div class="panel-body muted">{t('journal.no_match', { query })}</div>
          ) : (
            <div class="table-wrap">
              <table class="ledger cards">
                <thead>
                  <tr>
                    <th scope="col">{t('term.entry')}</th>
                    <th scope="col">{t('term.posting_date')}</th>
                    <th scope="col">{t('term.description')}</th>
                    <th scope="col">{t('entry.posted_by')}</th>
                    <th scope="col" class="num">
                      {t('term.amount')}
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {entries.map((entry) => {
                    const total = entryTotal(entry);
                    return (
                      <tr key={entry.entry_id}>
                        <td class="code" data-label={t('term.entry')}>
                          <a class="row-link" href={`#/journal/${encodeURIComponent(entry.external_id ?? entry.entry_id)}`}>
                            <Ltr>{entryRef(entry)}</Ltr>
                          </a>
                          {entry.reversed_by && (
                            <span class="badge reversed" style={{ marginLeft: '8px' }}>
                              {t('journal.reversed')}
                            </span>
                          )}
                        </td>
                        <td data-label={t('term.posting_date')} class="mono">
                          <Ltr>{formatDate(entry.posting_date)}</Ltr>
                        </td>
                        <td class="main desc">{entry.description}</td>
                        <td data-label={t('entry.posted_by')}>
                          <Marker by={entry.posted_by} />
                        </td>
                        <td class="num" data-label={t('term.amount')}>
                          <Money minor={total.minor} currency={total.currency} />
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}
        </section>
      )}
    </>
  );
}

/** One entry with its lines, and the reversal action (a new entry, never an edit). */
export function EntryDetail({ entryId }: { entryId: string }) {
  const { client, online, toast } = useSession();
  const list = useAsync(async () => (await entriesList(client)).entries, [client.baseUrl, client.apiKey]);
  const listed = list.data?.find((entry) => entry.external_id === entryId || entry.entry_id === entryId);
  const fresh = useAsync(
    async () => (listed?.external_id ? await entriesGet(client, listed.external_id) : listed ?? null),
    [client.baseUrl, client.apiKey, entryId, listed?.external_id],
  );
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const [postingDate, setPostingDate] = useState(todayIso());

  if (list.loading && !list.data) return <Skeleton rows={6} label={t('entry.loading')} />;
  if (list.error && !list.data) return <ErrorBanner error={list.error} onRetry={list.reload} />;
  if (!listed) {
    return (
      <Empty
        title={t('entry.not_found_title')}
        action={
          <a class="btn" href="#/journal">
            {t('entry.back_journal')}
          </a>
        }
      >
        {t('entry.not_found_body')}
      </Empty>
    );
  }
  const entry: EntryRecord = fresh.data ?? listed;
  const total = entryTotal(entry);
  const reversible = entry.external_id !== null && entry.reversed_by === null;
  const ref = entryRef(entry);
  const locale = intlLocale();

  const reverse = async () => {
    if (!entry.external_id) return;
    setBusy(true);
    setError(null);
    try {
      const result = await entriesReverse(client, entry.external_id, {
        posting_date: postingDate,
        description: `Reversal of ${ref}: ${entry.description}`,
      });
      setConfirming(false);
      toast(t('entry.reversal_posted', { ref: entryRef(result.entry) }));
      window.location.hash = '#/journal';
    } catch (err) {
      setError(err);
    } finally {
      setBusy(false);
    }
  };

  const reverseDisabledReason = !online
    ? t('common.offline_posting')
    : !entry.external_id
      ? t('entry.no_external_id')
      : entry.reversed_by
        ? t('entry.already_reversed')
        : undefined;

  return (
    <>
      <PageHead
        title={t('entry.title', { ref })}
        actions={
          <>
            <a class="btn btn-ghost" href="#/journal">
              {t('entry.back')}
            </a>
            <button
              type="button"
              class="btn btn-danger"
              disabled={!reversible || !online}
              title={reverseDisabledReason}
              onClick={() => {
                setError(null);
                setConfirming(true);
              }}
            >
              {t('entry.reverse')}
            </button>
          </>
        }
      >
        {entry.description}
      </PageHead>

      {entry.reversed_by && (
        <div class="banner" role="note">
          <span class="grow">{t('entry.reversed_by', { by: entry.reversed_by })}</span>
        </div>
      )}

      <section class="panel">
        <div class="panel-body">
          <dl class="entry-meta">
            <div>
              <dt>{t('entry.posted_by')}</dt>
              <dd>
                <Marker by={entry.posted_by} />
              </dd>
            </div>
            <div>
              <dt>{t('term.posting_date')}</dt>
              <dd>
                <Ltr>{formatDate(entry.posting_date)}</Ltr>
              </dd>
            </div>
            <div>
              <dt>{t('entry.document_date')}</dt>
              <dd>
                <Ltr>{formatDate(entry.document_date)}</Ltr>
              </dd>
            </div>
            <div>
              <dt>{t('entry.journal')}</dt>
              <dd>
                <Ltr>{entry.journal_id}</Ltr>
              </dd>
            </div>
            <div>
              <dt>{t('entry.recorded')}</dt>
              <dd>
                <Ltr>{entry.recorded_at}</Ltr>
              </dd>
            </div>
            <div>
              <dt>{t('entry.external_id')}</dt>
              <dd>
                <Ltr>{entry.external_id ?? t('common.empty_cell')}</Ltr>
              </dd>
            </div>
          </dl>
        </div>
        <div class="table-wrap">
          <table class="ledger cards">
            <caption class="sr-only">{t('entry.lines_caption', { ref })}</caption>
            <thead>
              <tr>
                <th scope="col">{t('term.account')}</th>
                <th scope="col">{t('term.memo')}</th>
                <th scope="col" class="num">
                  {t('term.debit')}
                </th>
                <th scope="col" class="num">
                  {t('term.credit')}
                </th>
              </tr>
            </thead>
            <tbody>
              {entry.lines.map((line, index) => (
                <tr key={index}>
                  <td class="code main" data-label={t('term.account')}>
                    <Ltr>{line.account_number}</Ltr>
                  </td>
                  <td class="muted" data-label={t('term.memo')}>
                    {line.memo ?? null}
                  </td>
                  <td class="num" data-label={t('term.debit')}>
                    {line.side === 'debit' ? <Money minor={line.base.minor} currency={line.base.currency} /> : null}
                  </td>
                  <td class="num" data-label={t('term.credit')}>
                    {line.side === 'credit' ? <Money minor={line.base.minor} currency={line.base.currency} /> : null}
                  </td>
                </tr>
              ))}
            </tbody>
            <tfoot>
              <tr>
                <td class="main" colSpan={2}>
                  {t('entry.total', { currency: total.currency })}
                </td>
                <td class="num" data-label={t('entry.debit_total')}>
                  <Money minor={total.minor} currency={total.currency} />
                </td>
                <td class="num" data-label={t('entry.credit_total')}>
                  <Money minor={total.minor} currency={total.currency} />
                </td>
              </tr>
            </tfoot>
          </table>
        </div>
      </section>

      <ConfirmDialog
        open={confirming}
        onClose={() => !busy && setConfirming(false)}
        title={t('entry.reverse_title', { ref })}
        confirmLabel={t('entry.reverse_confirm')}
        danger
        busy={busy}
        error={error}
        onConfirm={reverse}
      >
        <p>{tRich('entry.reverse_body', { reversal: <strong>{t('entry.reversal_entry')}</strong>, ref: <Ltr>{ref}</Ltr> })}</p>
        <label class="field">
          <span>{t('entry.reversal_date')}</span>
          <input class="input mono" type="date" value={postingDate} onInput={(event) => setPostingDate((event.target as HTMLInputElement).value)} />
        </label>
        <p class="muted">{t('entry.amount_reversed', { amount: formatMinor(total.minor, total.currency, locale) + ' ' + total.currency })}</p>
      </ConfirmDialog>
    </>
  );
}
