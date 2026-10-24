/**
 * Customers and vendors: the list. Search, role and review filters go to the
 * server; each row opens the party's page. Open payables come from one
 * balances read, so the list costs two calls however many rows it shows.
 */
import { useEffect, useMemo, useState } from 'preact/hooks';
import { partiesBalances, partiesList, type Party, type PartyBalance } from '../api/parties';
import { useAsync } from '../hooks';
import { t } from '../i18n';
import { Ltr } from '../i18n/Ltr';
import { useSession } from '../session';
import { Empty, ErrorBanner, Money, PageHead, Skeleton } from './common';
import { Select } from './Select';
import { PAGE_SIZE, REVIEW_FILTERS, ROLE_FILTERS, listQuery, roleLabelKey, vatStatus, type ReviewFilter, type RoleFilter } from '../lib/parties';
import { VatChip } from './PartyParts';
import { Avatar } from './Avatar';
import { partyExport } from '../api/parties';

/** A search that waits for a pause in typing before it asks the server. */
function useDebounced(value: string, ms: number): string {
  const [shown, setShown] = useState(value);
  useEffect(() => {
    const timer = setTimeout(() => setShown(value), ms);
    return () => clearTimeout(timer);
  }, [value, ms]);
  return shown;
}

/** The open payables of one party, one money figure per currency. */
export function payablesByCurrency(balances: PartyBalance[], partyId: string): { currency: string; minor: number; bills: number }[] {
  return balances.filter((b) => b.party_id === partyId).map((b) => ({ currency: b.currency, minor: b.payable_minor, bills: b.open_bills }));
}

export function Parties() {
  const { client, toast } = useSession();
  const [search, setSearch] = useState('');
  const [roles, setRoles] = useState<RoleFilter>('all');
  const [review, setReview] = useState<ReviewFilter>('all');
  const [page, setPage] = useState(0);
  const q = useDebounced(search, 250);

  useEffect(() => setPage(0), [q, roles, review]);

  const query = useMemo(() => listQuery({ q, roles, review }, page), [q, roles, review, page]);
  const list = useAsync(() => partiesList(client, query), [JSON.stringify(query)]);
  const balances = useAsync(() => partiesBalances(client), []);

  const data = list.data;
  const total = data?.total ?? 0;
  const from = total === 0 ? 0 : page * PAGE_SIZE + 1;
  const to = Math.min(total, (page + 1) * PAGE_SIZE);
  const balanceRows = balances.data?.balances ?? [];
  const [exporting, setExporting] = useState(false);

  /** Downloads every party as CSV, in the columns the import reads. */
  const exportCsv = async () => {
    setExporting(true);
    try {
      const out = await partyExport(client);
      const url = URL.createObjectURL(new Blob([out.csv], { type: 'text/csv;charset=utf-8' }));
      const link = document.createElement('a');
      link.href = url;
      link.download = 'parties.csv';
      document.body.appendChild(link);
      link.click();
      link.remove();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
      toast(t('parties.export_done'), 'ok');
    } catch (error) {
      toast(error instanceof Error ? error.message : t('parties.export_done'), 'error');
    } finally {
      setExporting(false);
    }
  };

  return (
    <>
      <PageHead
        title={t('parties.title')}
        actions={
          <>
            <button type="button" class="btn" onClick={() => void exportCsv()} disabled={exporting}>
              {t('parties.export')}
            </button>
            <a class="btn" href="#/parties/import">
              {t('parties.import')}
            </a>
            <a class="btn btn-primary" href="#/parties/new">
              {t('parties.new')}
            </a>
          </>
        }
      >
        {t('parties.intro')}
      </PageHead>

      <form class="party-filters" role="search" aria-labelledby="parties-filters-title" onSubmit={(event) => event.preventDefault()}>
        <h2 id="parties-filters-title" class="sr-only">
          {t('parties.filters')}
        </h2>
        <div class="field party-search">
          <label for="parties-search">{t('parties.search')}</label>
          <input
            id="parties-search"
            class="input"
            type="search"
            value={search}
            placeholder={t('parties.search_placeholder')}
            onInput={(event) => setSearch((event.target as HTMLInputElement).value)}
            autoComplete="off"
          />
        </div>
        <div class="field">
          <label id="parties-roles-label" for="parties-roles">
            {t('parties.filter_roles')}
          </label>
          <Select
            id="parties-roles"
            labelledBy="parties-roles-label"
            value={roles}
            onChange={(value) => setRoles(value as RoleFilter)}
            options={ROLE_FILTERS.map((f) => ({ value: f, label: f === 'all' ? t('parties.roles_all') : t(roleFilterKey(f)) }))}
          />
        </div>
        <div class="field">
          <label id="parties-review-label" for="parties-review">
            {t('parties.filter_review')}
          </label>
          <Select
            id="parties-review"
            labelledBy="parties-review-label"
            value={review}
            onChange={(value) => setReview(value as ReviewFilter)}
            options={REVIEW_FILTERS.map((f) => ({ value: f, label: reviewLabel(f) }))}
          />
        </div>
      </form>

      <div role="status" aria-live="polite" class="sr-only">
        {data ? t('parties.count', { total }) : ''}
      </div>

      {list.loading && !data && <Skeleton rows={6} label={t('parties.loading')} />}
      {list.error && !data && <ErrorBanner error={list.error} onRetry={list.reload} />}
      {data && data.parties.length === 0 && (
        <Empty title={t(q || roles !== 'all' || review !== 'all' ? 'parties.none_match' : 'parties.empty_title')}>
          {t(q || roles !== 'all' || review !== 'all' ? 'parties.none_match_body' : 'parties.empty_body')}
        </Empty>
      )}
      {data && data.parties.length > 0 && (
        <>
          <div class="table-wrap">
            <table class="ledger cards parties-table">
              <caption class="sr-only">{t('parties.title')}</caption>
              <thead>
                <tr>
                  <th scope="col">{t('parties.col_name')}</th>
                  <th scope="col">{t('parties.col_roles')}</th>
                  <th scope="col">{t('parties.col_vat')}</th>
                  <th scope="col" class="num-col">
                    {t('parties.col_payable')}
                  </th>
                  <th scope="col">{t('parties.col_review')}</th>
                </tr>
              </thead>
              <tbody>
                {data.parties.map((party) => (
                  <PartyRow key={party.id} party={party} balances={balanceRows} balancesLoading={balances.loading && !balances.data} />
                ))}
              </tbody>
            </table>
          </div>
          <nav class="party-pager" aria-label={t('parties.pages')}>
            <span class="muted">{t('parties.showing', { from, to, total })}</span>
            <button type="button" class="btn btn-sm" onClick={() => setPage((p) => Math.max(0, p - 1))} disabled={page === 0}>
              {t('parties.previous')}
            </button>
            <button type="button" class="btn btn-sm" onClick={() => setPage((p) => p + 1)} disabled={to >= total}>
              {t('parties.next')}
            </button>
          </nav>
        </>
      )}
      {list.error && data && <ErrorBanner error={list.error} onRetry={list.reload} />}
    </>
  );
}

function roleFilterKey(filter: RoleFilter): 'parties.role_customer' | 'parties.role_vendor' | 'parties.role_both' {
  if (filter === 'customer') return 'parties.role_customer';
  if (filter === 'both') return 'parties.role_both';
  return 'parties.role_vendor';
}

function reviewLabel(filter: ReviewFilter): string {
  if (filter === 'needs') return t('parties.review_needs');
  if (filter === 'reviewed') return t('parties.review_done');
  return t('parties.review_any');
}

function PartyRow({ party, balances, balancesLoading }: { party: Party; balances: PartyBalance[]; balancesLoading: boolean }) {
  const payables = payablesByCurrency(balances, party.id);
  return (
    <tr>
      <td class="main" data-label={t('parties.col_name')}>
        <a class="party-name" href={`#/parties/${encodeURIComponent(party.id)}`}>
          <Avatar name={party.name} logo={party.logo_document_id ? { documentId: party.logo_document_id } : null} size="sm" />
          <Ltr>{party.name}</Ltr>
        </a>
        {party.merged_into && <span class="badge state-soft">{t('parties.merged')}</span>}
      </td>
      <td data-label={t('parties.col_roles')}>
        <span class="badge party-role">{t(roleLabelKey(party.roles))}</span>
      </td>
      <td data-label={t('parties.col_vat')}>
        <VatChip status={vatStatus(party)} />
      </td>
      <td class="num-col" data-label={t('parties.col_payable')}>
        {balancesLoading ? (
          <span class="muted">{t('parties.loading_short')}</span>
        ) : payables.length === 0 ? (
          <span class="muted">{t('parties.nothing_open')}</span>
        ) : (
          payables.map((p) => <Money key={p.currency} minor={String(p.minor)} currency={p.currency} class="party-money" />)
        )}
      </td>
      <td data-label={t('parties.col_review')}>
        {party.needs_review ? <span class="badge state-open">{t('parties.needs_review')}</span> : <span class="muted">{t('parties.review_done')}</span>}
      </td>
    </tr>
  );
}
