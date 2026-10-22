/**
 * One bank account: its balances, its lines with filters, and the reconcile
 * panel for a selected line. Lines page with the server's cursor (`next`).
 */
import { useCallback, useEffect, useRef, useState } from 'preact/hooks';
import { bankAccountGet, bankTransactionsList, type AccountView, type TransactionView } from '../api/bank';
import { useAsync } from '../hooks';
import { useSession } from '../session';
import { EMPTY_FILTERS, filtersToQuery, formatIban, lineStatus, LINE_STATUSES, type LineStatus, type TransactionFilters } from '../lib/bank';
import { formatDate, t, type MessageKey } from '../i18n';
import { Empty, ErrorBanner, Money, PageHead, Skeleton } from './common';
import { BankImport } from './BankImport';
import { BankReconcile } from './BankReconcile';
import { DifferenceValue, SignedAmount, StatusChip } from './BankShared';
import { Icon } from './icons';
import { Select } from './Select';

const FILTER_KEYS: Record<'all' | LineStatus, MessageKey> = {
  all: 'bank.filter_all',
  unmatched: 'bank.status_unmatched',
  suggested: 'bank.status_suggested',
  matched: 'bank.status_matched',
  ignored: 'bank.status_ignored',
};

export function BankAccountPage({ id }: { id: string }) {
  const { client } = useSession();
  const account = useAsync(() => bankAccountGet(client, id), [client.baseUrl, client.apiKey, id]);
  const [applied, setApplied] = useState<TransactionFilters>(EMPTY_FILTERS);
  const [searchText, setSearchText] = useState('');
  const [lines, setLines] = useState<TransactionView[]>([]);
  const [next, setNext] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [importOpen, setImportOpen] = useState(false);
  const generation = useRef(0);

  // The search box applies after a short pause, so typing does not call the server per key.
  useEffect(() => {
    const timer = window.setTimeout(() => setApplied((current) => (current.search === searchText ? current : { ...current, search: searchText })), 300);
    return () => window.clearTimeout(timer);
  }, [searchText]);

  const fetchPage = useCallback(
    (after?: string) => {
      const current = ++generation.current;
      setLoading(true);
      bankTransactionsList(client, filtersToQuery(applied, id, { after }))
        .then((page) => {
          if (current !== generation.current) return;
          setLines((existing) => (after ? [...existing, ...page.transactions] : page.transactions));
          setNext(page.next);
          setError(null);
        })
        .catch((err: unknown) => {
          if (current === generation.current) setError(err);
        })
        .finally(() => {
          if (current === generation.current) setLoading(false);
        });
    },
    [client, applied, id],
  );

  // A new filter or account starts again from the first page.
  useEffect(() => {
    setLines([]);
    setNext(null);
    fetchPage();
  }, [fetchPage]);

  /** After a write: the account's balances and the first page both change. */
  const refresh = () => {
    account.reload();
    fetchPage();
  };

  const selected = lines.find((line) => line.id === selectedId) ?? null;
  const acc: AccountView | undefined = account.data;

  if (account.error !== undefined && account.error !== null) {
    return (
      <>
        <a class="back-link" href="#/bank">
          <Icon name="chevron-left" />
          {t('bank.back')}
        </a>
        <ErrorBanner error={account.error} onRetry={account.reload} />
      </>
    );
  }
  if (!acc) return <Skeleton rows={4} label={t('bank.loading')} />;

  return (
    <>
      <a class="back-link bank-back" href="#/bank">
        <Icon name="chevron-left" />
        {t('bank.back')}
      </a>
      <PageHead
        title={acc.name}
        actions={
          <button type="button" class="btn btn-primary" onClick={() => setImportOpen(true)}>
            {t('bank.import_button')}
          </button>
        }
      >
        <span class="mono bank-head-iban">{formatIban(acc.iban)}</span> <span class="chip">{acc.currency}</span>
      </PageHead>

      <div class="tiles bank-tiles">
        <div class="tile">
          <span class="label">
            {t('bank.statement_balance')}
            {acc.statement_balance ? ` · ${formatDate(acc.statement_balance.date)}` : ''}
          </span>
          <span class="value">
            {acc.statement_balance ? (
              <Money minor={acc.statement_balance.amount.minor} currency={acc.statement_balance.amount.currency} />
            ) : (
              <span class="muted">{t('bank.no_statement')}</span>
            )}
          </span>
        </div>
        <div class="tile">
          <span class="label">{t('bank.ledger_balance')}</span>
          <span class="value">
            <Money minor={acc.ledger_balance.minor} currency={acc.ledger_balance.currency} />
          </span>
        </div>
        <div class="tile">
          <span class="label">{t('bank.difference')}</span>
          <span class="value">
            <DifferenceValue minor={acc.difference.minor} currency={acc.difference.currency} />
          </span>
        </div>
        <div class="tile">
          <span class="label">{t('bank.counts_label')}</span>
          <span class="sub">{t('bank.counts', { unmatched: acc.unmatched_count, matched: acc.matched_count, ignored: acc.ignored_count })}</span>
        </div>
      </div>

      <div class="bank-body">
        <section class="panel bank-lines-panel" aria-labelledby="bank-lines-title">
          <div class="panel-head">
            <h2 id="bank-lines-title">{t('bank.lines')}</h2>
          </div>
          <form class="bank-filters" role="search" aria-label={t('bank.filters')} onSubmit={(event) => event.preventDefault()}>
            <div class="field">
              <span id="bank-filter-status-label">{t('bank.filter_status')}</span>
              <Select
                id="bank-filter-status"
                labelledBy="bank-filter-status-label"
                value={applied.status}
                searchable={false}
                onChange={(value) => setApplied((current) => ({ ...current, status: value as TransactionFilters['status'] }))}
                options={(['all', ...LINE_STATUSES] as const).map((status) => ({ value: status, label: t(FILTER_KEYS[status]) }))}
              />
            </div>
            <label class="field bank-filter-search">
              <span>{t('bank.filter_search')}</span>
              <input
                id="bank-filter-search"
                class="input"
                type="search"
                autoComplete="off"
                value={searchText}
                onInput={(event) => setSearchText((event.target as HTMLInputElement).value)}
              />
            </label>
            <label class="field">
              <span>{t('bank.filter_from')}</span>
              <input
                id="bank-filter-from"
                class="input"
                type="date"
                value={applied.from}
                onInput={(event) => setApplied((current) => ({ ...current, from: (event.target as HTMLInputElement).value }))}
              />
            </label>
            <label class="field">
              <span>{t('bank.filter_to')}</span>
              <input
                id="bank-filter-to"
                class="input"
                type="date"
                value={applied.to}
                onInput={(event) => setApplied((current) => ({ ...current, to: (event.target as HTMLInputElement).value }))}
              />
            </label>
          </form>

          {error !== null && (
            <div class="panel-body">
              <ErrorBanner error={error} onRetry={() => fetchPage()} />
            </div>
          )}
          {lines.length === 0 && loading && <Skeleton rows={4} label={t('bank.loading')} />}
          {lines.length === 0 && !loading && error === null && <Empty title={t('bank.lines_empty_title')}>{t('bank.lines_empty_body')}</Empty>}

          {lines.length > 0 && (
            <ul class="bank-lines" aria-label={t('bank.lines')}>
              {lines.map((line) => (
                <LineRow key={line.id} line={line} selected={line.id === selectedId} currency={acc.currency} onSelect={() => setSelectedId(line.id)} />
              ))}
            </ul>
          )}
          {next && (
            <div class="bank-more">
              <button type="button" class="btn" disabled={loading} aria-busy={loading ? 'true' : undefined} onClick={() => fetchPage(next)}>
                {loading ? t('common.working') : t('bank.load_more')}
              </button>
            </div>
          )}
        </section>

        <aside class="panel bank-reconcile-panel" aria-labelledby="bank-reconcile-title" id="bank-reconcile">
          <div class="panel-head">
            <h2 id="bank-reconcile-title">{t('bank.reconcile_title')}</h2>
          </div>
          <div class="panel-body">
            {selected ? (
              <BankReconcile key={selected.id} account={acc} line={selected} onChanged={refresh} />
            ) : (
              <p class="muted">{t('bank.reconcile_pick')}</p>
            )}
          </div>
        </aside>
      </div>

      <BankImport account={acc} open={importOpen} onClose={() => setImportOpen(false)} onImported={refresh} />
    </>
  );
}

function LineRow({ line, selected, currency, onSelect }: { line: TransactionView; selected: boolean; currency: string; onSelect: () => void }) {
  const status = lineStatus(line);
  const flagged = line.flags.includes('remittance-reads-like-instruction');
  return (
    <li>
      <button type="button" class="bank-line" aria-pressed={selected ? 'true' : 'false'} onClick={onSelect}>
        <span class="bank-line-date mono">{formatDate(line.booking_date)}</span>
        <span class="bank-line-main">
          <strong>{line.counterparty_name || line.counterparty_iban || t('bank.no_counterparty')}</strong>
          <span class="bank-line-remit">{line.remittance || t('bank.no_remittance')}</span>
          {flagged && (
            <span class="bank-flag">
              <Icon name="info" />
              {t('bank.flag_short')}
            </span>
          )}
        </span>
        <SignedAmount minor={line.amount.minor} currency={line.amount.currency || currency} />
        <StatusChip status={status} />
      </button>
    </li>
  );
}
