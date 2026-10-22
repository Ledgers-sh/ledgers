import { useMemo, useRef, useState } from 'preact/hooks';
import { entriesGet, entriesList, entriesReverse } from '../api/commands';
import type { EntryRecord } from '../api/types';
import { useAsync } from '../hooks';
import { useSession } from '../session';
import { todayIso } from '../lib/period';
import { ConfirmDialog, Empty, ErrorBanner, LOCALE, Marker, Money, PageHead, Skeleton } from './common';
import { formatMinor } from '../lib/money';

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

  if (state.loading && !state.data) return <Skeleton rows={6} label="Loading journal" />;
  if (state.error && !state.data) return <ErrorBanner error={state.error} onRetry={state.reload} />;
  const all = state.data ?? [];

  return (
    <>
      <PageHead
        title="Journal"
        actions={
          <a class="btn btn-primary" href="#/new">
            New entry
          </a>
        }
      >
        Posted entries, newest first. Entries are never edited; a correction is a reversal plus a new entry.
      </PageHead>

      {all.length === 0 ? (
        <Empty
          title="No entries yet"
          action={
            <a class="btn btn-primary" href="#/new">
              Post your first entry
            </a>
          }
        >
          The journal lists every posted entry, whether an agent or a person posted it.
        </Empty>
      ) : (
        <section class="panel" aria-label="Entries">
          <div class="panel-head">
            <label class="sr-only" for="journal-search">
              Search entries
            </label>
            <input
              id="journal-search"
              ref={searchRef}
              class="input"
              type="search"
              placeholder="Search description or number (press /)"
              value={query}
              onInput={(event) => setQuery((event.target as HTMLInputElement).value)}
              style={{ maxWidth: '360px' }}
            />
            <span class="grow" />
            <span class="muted">{entries.length} of {all.length}</span>
          </div>
          {entries.length === 0 ? (
            <div class="panel-body muted">No entries match “{query}”.</div>
          ) : (
            <div class="table-wrap">
              <table class="ledger cards">
                <thead>
                  <tr>
                    <th scope="col">Entry</th>
                    <th scope="col">Posting date</th>
                    <th scope="col">Description</th>
                    <th scope="col">Posted by</th>
                    <th scope="col" class="num">
                      Amount
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {entries.map((entry) => {
                    const total = entryTotal(entry);
                    return (
                      <tr key={entry.entry_id}>
                        <td class="code" data-label="Entry">
                          <a class="row-link" href={`#/journal/${encodeURIComponent(entry.external_id ?? entry.entry_id)}`}>
                            {entryRef(entry)}
                          </a>
                          {entry.reversed_by && (
                            <span class="badge reversed" style={{ marginLeft: '8px' }}>
                              reversed
                            </span>
                          )}
                        </td>
                        <td data-label="Posting date" class="mono">
                          {entry.posting_date}
                        </td>
                        <td class="main desc">{entry.description}</td>
                        <td data-label="Posted by">
                          <Marker by={entry.posted_by} />
                        </td>
                        <td class="num" data-label="Amount">
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

  if (list.loading && !list.data) return <Skeleton rows={6} label="Loading entry" />;
  if (list.error && !list.data) return <ErrorBanner error={list.error} onRetry={list.reload} />;
  if (!listed) {
    return (
      <Empty
        title="Entry not found"
        action={
          <a class="btn" href="#/journal">
            Back to the journal
          </a>
        }
      >
        It may belong to another period or ledger.
      </Empty>
    );
  }
  const entry: EntryRecord = fresh.data ?? listed;
  const total = entryTotal(entry);
  const reversible = entry.external_id !== null && entry.reversed_by === null;

  const reverse = async () => {
    if (!entry.external_id) return;
    setBusy(true);
    setError(null);
    try {
      const result = await entriesReverse(client, entry.external_id, {
        posting_date: postingDate,
        description: `Reversal of ${entryRef(entry)}: ${entry.description}`,
      });
      setConfirming(false);
      toast(`Reversal posted as ${entryRef(result.entry)}.`);
      window.location.hash = '#/journal';
    } catch (err) {
      setError(err);
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <PageHead
        title={`Entry ${entryRef(entry)}`}
        actions={
          <>
            <a class="btn btn-ghost" href="#/journal">
              Back to journal
            </a>
            <button
              type="button"
              class="btn btn-danger"
              disabled={!reversible || !online}
              title={
                !online
                  ? 'offline: posting needs the server'
                  : !entry.external_id
                    ? 'This entry has no external ID, so it cannot be reversed here.'
                    : entry.reversed_by
                      ? 'Already reversed.'
                      : undefined
              }
              onClick={() => {
                setError(null);
                setConfirming(true);
              }}
            >
              Reverse entry
            </button>
          </>
        }
      >
        {entry.description}
      </PageHead>

      {entry.reversed_by && (
        <div class="banner" role="note">
          <span class="grow">This entry was reversed by {entry.reversed_by}. The original stays in the journal unchanged.</span>
        </div>
      )}

      <section class="panel">
        <div class="panel-body">
          <dl class="entry-meta">
            <div>
              <dt>Posted by</dt>
              <dd>
                <Marker by={entry.posted_by} />
              </dd>
            </div>
            <div>
              <dt>Posting date</dt>
              <dd>{entry.posting_date}</dd>
            </div>
            <div>
              <dt>Document date</dt>
              <dd>{entry.document_date}</dd>
            </div>
            <div>
              <dt>Journal</dt>
              <dd>{entry.journal_id}</dd>
            </div>
            <div>
              <dt>Recorded</dt>
              <dd>{entry.recorded_at}</dd>
            </div>
            <div>
              <dt>External ID</dt>
              <dd>{entry.external_id ?? '—'}</dd>
            </div>
          </dl>
        </div>
        <div class="table-wrap">
          <table class="ledger cards">
            <caption class="sr-only">Lines of entry {entryRef(entry)}</caption>
            <thead>
              <tr>
                <th scope="col">Account</th>
                <th scope="col">Memo</th>
                <th scope="col" class="num">
                  Debit
                </th>
                <th scope="col" class="num">
                  Credit
                </th>
              </tr>
            </thead>
            <tbody>
              {entry.lines.map((line, index) => (
                <tr key={index}>
                  <td class="code main" data-label="Account">
                    {line.account_number}
                  </td>
                  <td class="muted" data-label="Memo">
                    {line.memo ?? null}
                  </td>
                  <td class="num" data-label="Debit">
                    {line.side === 'debit' ? <Money minor={line.base.minor} currency={line.base.currency} /> : null}
                  </td>
                  <td class="num" data-label="Credit">
                    {line.side === 'credit' ? <Money minor={line.base.minor} currency={line.base.currency} /> : null}
                  </td>
                </tr>
              ))}
            </tbody>
            <tfoot>
              <tr>
                <td class="main" colSpan={2}>
                  Total {total.currency}
                </td>
                <td class="num" data-label="Debit total">
                  <Money minor={total.minor} currency={total.currency} />
                </td>
                <td class="num" data-label="Credit total">
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
        title={`Reverse ${entryRef(entry)}?`}
        confirmLabel="Post reversal"
        danger
        busy={busy}
        error={error}
        onConfirm={reverse}
      >
        <p>
          This posts a <strong>new reversal entry</strong> that offsets {entryRef(entry)} line by line. The original entry is
          not edited or deleted.
        </p>
        <label class="field">
          <span>Reversal posting date</span>
          <input class="input mono" type="date" value={postingDate} onInput={(event) => setPostingDate((event.target as HTMLInputElement).value)} />
        </label>
        <p class="muted">
          Amount reversed: {formatMinor(total.minor, total.currency, LOCALE)} {total.currency}.
        </p>
      </ConfirmDialog>
    </>
  );
}
