/**
 * One party: the profile, addresses, contacts and payment terms; the bank
 * details with their history (owners and admins change them, with a reason);
 * the bills and bank lines that name the party; open payables; the possible
 * duplicates; and the merge (owners and admins).
 */
import { useState } from 'preact/hooks';
import { partyCheckVat, partyGet, partyOpenItems, type PartyDetail, type PartyOpenItems } from '../api/parties';
import { billsList, type Bill } from '../api/bills';
import { bankTransactionsList, type TransactionView } from '../api/bank';
import { useAsync } from '../hooks';
import { formatDate, t } from '../i18n';
import { Ltr } from '../i18n/Ltr';
import { useSession } from '../session';
import { Empty, ErrorBanner, Money, PageHead, Skeleton } from './common';
import { FormProblem } from './settings/common';
import { ReadValue } from './PartyFields';
import { VatChip } from './PartyParts';
import { BankChangeDialog, MergeDialog } from './PartyDialogs';
import { Avatar } from './Avatar';
import { bankChangeLines, canManageBank, compactVat, formatIban, roleLabelKey, vatCountry, vatStatus } from '../lib/parties';

export function PartyPage({ id }: { id: string }) {
  const { client, person } = useSession();
  const detail = useAsync<PartyDetail>(() => partyGet(client, id), [id]);
  const open = useAsync<PartyOpenItems>(() => partyOpenItems(client, id), [id]);
  const bills = useAsync(() => billsList(client), [id]);
  const lines = useAsync(() => bankTransactionsList(client, { limit: 200 }), [id]);

  if (detail.loading && !detail.data) return <Skeleton rows={8} label={t('parties.loading')} />;
  if (detail.error && !detail.data) return <ErrorBanner error={detail.error} onRetry={detail.reload} />;
  if (!detail.data) return null;
  return (
    <PartyView
      key={`${detail.data.party.id}-${detail.data.party.name}-${detail.data.bank_changes.length}-${detail.data.party.logo_document_id ?? ''}`}
      detail={detail.data}
      reload={detail.reload}
      role={person?.role ?? null}
      open={open.data ?? null}
      openError={open.error}
      billRows={bills.data?.bills ?? null}
      billsError={bills.error}
      lines={lines.data?.transactions ?? null}
    />
  );
}

function PartyView({
  detail,
  reload,
  role,
  open,
  openError,
  billRows,
  billsError,
  lines,
}: {
  detail: PartyDetail;
  reload: () => void;
  role: string | null;
  open: PartyOpenItems | null;
  openError: unknown;
  billRows: Bill[] | null;
  billsError: unknown;
  lines: TransactionView[] | null;
}) {
  const { client, toast } = useSession();
  const { party } = detail;
  const [bankOpen, setBankOpen] = useState(false);
  const [mergeOpen, setMergeOpen] = useState(false);
  const [vatBusy, setVatBusy] = useState(false);
  const [vatProblem, setVatProblem] = useState<unknown>(null);
  const merged = party.merged_into !== null;
  const canBank = canManageBank(role) && !merged;
  const canMerge = canManageBank(role) && !merged;
  const status = vatStatus(party);
  const linkedBills = (billRows ?? []).filter((b) => b.vendor.id === party.id);
  const linkedLines = (lines ?? []).filter((line) => line.party?.id === party.id);
  const vatNumber = party.vat_id ? compactVat(party.vat_id) : '';
  const vatCountryCode = party.vat_id ? vatCountry(party.vat_id) : null;

  const checkVat = async () => {
    if (!party.vat_id || !vatCountryCode) return;
    setVatBusy(true);
    setVatProblem(null);
    try {
      const result = await partyCheckVat(client, party.id, vatCountryCode, vatNumber.slice(2));
      toast(result.vies_result.valid ? t('parties.vat_checked_valid') : t('parties.vat_checked_invalid'), result.vies_result.valid ? 'ok' : 'error');
      reload();
    } catch (error) {
      setVatProblem(error);
    } finally {
      setVatBusy(false);
    }
  };

  return (
    <>
      <div class="party-hero">
        <Avatar name={party.name} logo={party.logo_document_id ? { documentId: party.logo_document_id } : null} size="lg" />
        <PageHead title={party.name}>
          <span class="party-head-meta">
          <span class="badge party-role">{t(roleLabelKey(party.roles))}</span> <VatChip status={status} />
          {party.needs_review && <span class="badge state-open">{t('parties.needs_review')}</span>}
          {merged && <span class="badge state-soft">{t('parties.merged')}</span>}
        </span>
        </PageHead>
      </div>
      <nav class="back-link-row party-page-actions">
        <a class="back-link" href="#/parties">
          {t('parties.back')}
        </a>
        {!merged && (
          <a class="btn" href={`#/parties/${encodeURIComponent(party.id)}/edit`}>
            {t('parties.edit')}
          </a>
        )}
        {canMerge && (
          <button type="button" class="btn" onClick={() => setMergeOpen(true)}>
            {t('parties.merge_into')}
          </button>
        )}
      </nav>

      {merged && (
        <p class="banner" role="status">
          {t('parties.merged_notice')}{' '}
          <a href={`#/parties/${encodeURIComponent(party.merged_into ?? '')}`}>{t('parties.open_survivor')}</a>
        </p>
      )}

      {party.possible_duplicates.length > 0 && (
        <p class="banner" role="note">
          {t('parties.duplicates_notice', { n: party.possible_duplicates.length })}{' '}
          {party.possible_duplicates.map((dup, index) => (
            <span key={dup}>
              {index > 0 && ', '}
              <a href={`#/parties/${encodeURIComponent(dup)}`}>{dup}</a>
            </span>
          ))}
        </p>
      )}

      <div class="party-columns">
        <section class="panel party-section" aria-labelledby="party-profile-title">
          <h2 id="party-profile-title">{t('parties.profile')}</h2>
          <dl class="party-facts">
            <ReadValue label={t('parties.vat_id')} ltr>
              {party.vat_id ? <span class="mono">{party.vat_id}</span> : <span class="muted">{t('parties.none')}</span>}
            </ReadValue>
            <ReadValue label={t('parties.tax_number')} ltr>
              {party.tax_number ?? <span class="muted">{t('parties.none')}</span>}
            </ReadValue>
            <ReadValue label={t('parties.payment_terms')}>
              {party.payment_terms_days === null ? <span class="muted">{t('parties.none')}</span> : t('parties.terms_days', { n: party.payment_terms_days })}
            </ReadValue>
            <ReadValue label={t('parties.currency')} ltr>
              {party.currency ?? <span class="muted">{t('parties.none')}</span>}
            </ReadValue>
            <ReadValue label={t('parties.default_account')} ltr>
              {party.default_account ?? <span class="muted">{t('parties.none')}</span>}
            </ReadValue>
            <ReadValue label={t('parties.trading_name')} ltr>
              {party.trading_name ?? <span class="muted">{t('parties.none')}</span>}
            </ReadValue>
            <ReadValue label={t('parties.registration_number')} ltr>
              {party.registration_number ?? <span class="muted">{t('parties.none')}</span>}
            </ReadValue>
            <ReadValue label={t('parties.website')} ltr>
              {party.website ? (
                <a href={party.website} rel="noopener noreferrer" target="_blank">
                  <Ltr>{party.website}</Ltr>
                </a>
              ) : (
                <span class="muted">{t('parties.none')}</span>
              )}
            </ReadValue>
            <ReadValue label={t('parties.email')} ltr>
              {party.email ? (
                <a href={`mailto:${party.email}`}>
                  <Ltr>{party.email}</Ltr>
                </a>
              ) : (
                <span class="muted">{t('parties.none')}</span>
              )}
            </ReadValue>
            <ReadValue label={t('parties.phone')} ltr>
              {party.phone ?? <span class="muted">{t('parties.none')}</span>}
            </ReadValue>
            <ReadValue label={t('parties.default_unit')} ltr>
              {party.default_unit ?? <span class="muted">{t('parties.none')}</span>}
            </ReadValue>
            <ReadValue label={t('parties.tags')}>
              {party.tags.length === 0 ? (
                <span class="muted">{t('parties.none')}</span>
              ) : (
                <span class="party-tags">
                  {party.tags.map((tag) => (
                    <span class="badge" key={tag}>
                      <Ltr>{tag}</Ltr>
                    </span>
                  ))}
                </span>
              )}
            </ReadValue>
            <ReadValue label={t('parties.notes')}>
              {party.notes ? <span class="party-notes-text">{party.notes}</span> : <span class="muted">{t('parties.none')}</span>}
            </ReadValue>
            <ReadValue label={t('parties.tax_ids')}>
              {party.tax_ids.length === 0 ? (
                <span class="muted">{t('parties.none')}</span>
              ) : (
                <ul class="party-plain-list">
                  {party.tax_ids.map((id) => (
                    <li key={`${id.country}-${id.value}`}>
                      <Ltr>{id.country}</Ltr> <span class="muted">{t(taxKindKey(id.kind))}</span> <span class="mono"><Ltr>{id.value}</Ltr></span>
                    </li>
                  ))}
                </ul>
              )}
            </ReadValue>
            <ReadValue label={t('parties.personal_data')}>{party.personal_data ? t('parties.yes') : t('parties.no')}</ReadValue>
          </dl>
          {party.vat_id && (
            <div class="party-vat-check">
              {detail.vies_enabled ? (
                <>
                  <button type="button" class="btn btn-sm" onClick={checkVat} disabled={vatBusy || merged || !vatCountryCode}>
                    {t('parties.check_vat')}
                  </button>
                  {party.latest_vies && (
                    <span class="muted">
                      {' '}
                      {t('parties.vat_last_checked', { date: formatDate(party.latest_vies.checked_at.slice(0, 10)) })}
                      {party.latest_vies.consultation_number && <> · <Ltr>{party.latest_vies.consultation_number}</Ltr></>}
                    </span>
                  )}
                </>
              ) : (
                <p class="muted" role="note">
                  {t('parties.vies_off')}
                </p>
              )}
              <FormProblem error={vatProblem} />
            </div>
          )}
        </section>

        <section class="panel party-section" aria-labelledby="party-bank-title">
          <h2 id="party-bank-title">{t('parties.bank')}</h2>
          <dl class="party-facts">
            <ReadValue label={t('parties.iban')} ltr>
              {party.iban ? <span class="mono">{formatIban(party.iban)}</span> : <span class="muted">{t('parties.none')}</span>}
            </ReadValue>
            <ReadValue label={t('parties.bic')} ltr>
              {party.bic ?? <span class="muted">{t('parties.none')}</span>}
            </ReadValue>
          </dl>
          {canBank ? (
            <button type="button" class="btn btn-sm" onClick={() => setBankOpen(true)}>
              {t('parties.bank_change')}
            </button>
          ) : (
            <p class="muted">{merged ? t('parties.bank_merged') : t('parties.bank_roles_only')}</p>
          )}
          <h3 class="party-subhead">{t('parties.bank_history')}</h3>
          {detail.bank_changes.length === 0 ? (
            <p class="muted">{t('parties.bank_history_none')}</p>
          ) : (
            <ol class="party-history" aria-label={t('parties.bank_history')}>
              {detail.bank_changes.map((change) => {
                const lines = bankChangeLines(change);
                return (
                  <li key={change.id}>
                    <time dateTime={change.changed_at}>{formatDate(change.changed_at.slice(0, 10))}</time>{' '}
                    <span class="mono">
                      <Ltr>{lines.before ?? t('parties.none')}</Ltr> → <Ltr>{lines.after ?? t('parties.none')}</Ltr>
                    </span>
                    <div class="muted">
                      {t('parties.changed_by', { who: change.changed_by })}
                      {change.reason ? <> · {change.reason}</> : null}
                    </div>
                  </li>
                );
              })}
            </ol>
          )}
        </section>
      </div>

      <div class="party-columns">
        <section class="panel party-section" aria-labelledby="party-addresses-title">
          <h2 id="party-addresses-title">{t('parties.addresses')}</h2>
          {party.addresses.length === 0 ? (
            <p class="muted">{t('parties.no_addresses')}</p>
          ) : (
            <ul class="party-cards">
              {party.addresses.map((a, index) => (
                <li key={index}>
                  <span class="badge">{a.kind ? t(kindKey(a.kind)) : t('parties.kind_other')}</span>
                  <address>
                    <Ltr>{a.line1}</Ltr>
                    {a.line2 && (
                      <>
                        <br />
                        <Ltr>{a.line2}</Ltr>
                      </>
                    )}
                    <br />
                    <Ltr>{[a.postal_code, a.city].filter(Boolean).join(' ')}</Ltr>
                    <br />
                    <Ltr>{a.country}</Ltr>
                  </address>
                </li>
              ))}
            </ul>
          )}
        </section>
        <section class="panel party-section" aria-labelledby="party-contacts-title">
          <h2 id="party-contacts-title">{t('parties.contacts')}</h2>
          {party.contacts.length === 0 ? (
            <p class="muted">{t('parties.no_contacts')}</p>
          ) : (
            <ul class="party-cards">
              {party.contacts.map((c, index) => (
                <li key={index}>
                  <strong>
                    <Ltr>{c.name}</Ltr>
                  </strong>
                  {c.primary && <span class="badge party-primary">{t('parties.primary_badge')}</span>}
                  {c.role && <span class="muted"> · {c.role}</span>}
                  <div>
                    {c.email && (
                      <a href={`mailto:${c.email}`}>
                        <Ltr>{c.email}</Ltr>
                      </a>
                    )}
                    {c.phone && (
                      <span>
                        {' '}
                        <Ltr>{c.phone}</Ltr>
                      </span>
                    )}
                  </div>
                </li>
              ))}
            </ul>
          )}
        </section>
      </div>

      <div class="party-columns">
        <section class="panel party-section" aria-labelledby="party-open-title">
          <h2 id="party-open-title">{t('parties.open_items')}</h2>
          {openError ? (
            <ErrorBanner error={openError} />
          ) : open === null ? (
            <p class="muted">{t('parties.loading_short')}</p>
          ) : open.open_items.length === 0 ? (
            <p class="muted">{t('parties.open_items_none')}</p>
          ) : (
            <>
              <ul class="party-cards party-balances">
                {open.balances.map((b) => (
                  <li key={`${b.ledger}-${b.currency}`}>
                    <span class="badge">{b.ledger === 'ar' ? t('parties.balance_ar') : t('parties.balance_ap')}</span>{' '}
                    <Money minor={String(b.remaining_minor)} currency={b.currency} />
                  </li>
                ))}
              </ul>
              <div class="table-wrap">
                <table class="ledger cards party-open-table">
                  <caption class="sr-only">{t('parties.open_items')}</caption>
                  <thead>
                    <tr>
                      <th scope="col">{t('parties.col_document')}</th>
                      <th scope="col">{t('parties.col_due')}</th>
                      <th scope="col" class="num-col">
                        {t('parties.col_remaining')}
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {open.open_items.map((item) => (
                      <tr key={item.id}>
                        <td class="main" data-label={t('parties.col_document')}>
                          <span class="badge">{item.ledger === 'ar' ? t('parties.open_receivable') : t('parties.open_payable')}</span>{' '}
                          <Ltr>{item.document ?? item.id}</Ltr>
                        </td>
                        <td data-label={t('parties.col_due')}>{formatDate((item.due_date ?? item.posting_date).slice(0, 10))}</td>
                        <td class="num-col" data-label={t('parties.col_remaining')}>
                          <Money minor={String(item.remaining_minor)} currency={item.currency} />
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </>
          )}
        </section>
        <section class="panel party-section" aria-labelledby="party-bills-title">
          <h2 id="party-bills-title">{t('parties.linked_bills')}</h2>
          <p class="muted">{t('parties.bill_count', { n: detail.bill_count })}</p>
          {billsError ? (
            <ErrorBanner error={billsError} />
          ) : linkedBills.length === 0 ? (
            detail.bill_count === 0 ? <p class="muted">{t('parties.no_linked_bills')}</p> : <p class="muted">{t('parties.bills_on_bills_page')}</p>
          ) : (
            <ul class="party-cards">
              {linkedBills.map((b) => (
                <li key={b.id}>
                  <a href={`#/bills/${encodeURIComponent(b.id)}`}>
                    <Ltr>{b.invoice_number ?? b.id}</Ltr>
                  </a>{' '}
                  <span class="muted">{billStatusText(b.status)}</span>
                  {b.gross_minor !== null && (
                    <>
                      {' '}
                      <Money minor={String(b.gross_minor)} currency={b.currency} />
                    </>
                  )}
                </li>
              ))}
            </ul>
          )}
        </section>
      </div>

      <div class="party-columns">
        <section class="panel party-section" aria-labelledby="party-recent-bills-title">
          <h2 id="party-recent-bills-title">{t('parties.recent_bills')}</h2>
          {detail.recent_bills.length === 0 ? (
            <p class="muted">{t('parties.no_linked_bills')}</p>
          ) : (
            <ul class="party-cards">
              {detail.recent_bills.map((b) => (
                <li key={b.id}>
                  <a href={`#/bills/${encodeURIComponent(b.id)}`}>
                    <Ltr>{b.invoice_number ?? b.id}</Ltr>
                  </a>{' '}
                  {b.gross_minor !== null && <Money minor={String(b.gross_minor)} currency={b.currency} />}{' '}
                  <span class="muted">{b.discarded ? t('parties.bill_status_discarded') : b.posted ? t('parties.bill_status_posted') : t('parties.bill_status_ready')}</span>
                </li>
              ))}
            </ul>
          )}
        </section>
        <section class="panel party-section" aria-labelledby="party-recent-payments-title">
          <h2 id="party-recent-payments-title">{t('parties.recent_payments')}</h2>
          {detail.recent_payments.length === 0 ? (
            <p class="muted">{t('parties.no_recent_payments')}</p>
          ) : (
            <ul class="party-cards">
              {detail.recent_payments.map((p) => (
                <li key={p.bill_id}>
                  <span class="muted">{formatDate(p.paid_at.slice(0, 10))}</span>{' '}
                  <Ltr>{t('parties.payment_line', { bill: p.invoice_number ?? p.bill_id })}</Ltr>{' '}
                  <Money minor={String(p.paid_minor)} currency={p.currency} />
                </li>
              ))}
            </ul>
          )}
        </section>
      </div>

      <section class="panel party-section" aria-labelledby="party-lines-title">
        <h2 id="party-lines-title">{t('parties.linked_lines')}</h2>
        {linkedLines.length === 0 ? (
          <p class="muted">{t('parties.no_linked_lines')}</p>
        ) : (
          <ul class="party-cards">
            {linkedLines.map((line) => (
              <li key={line.id}>
                <span class="muted">{formatDate(line.booking_date)}</span> <Ltr>{line.remittance || line.counterparty_name || line.id}</Ltr>{' '}
                <Money minor={String(line.amount.minor)} currency={line.amount.currency} />
              </li>
            ))}
          </ul>
        )}
      </section>

      {bankOpen && (
        <BankChangeDialog
          party={party}
          open={bankOpen}
          onClose={() => setBankOpen(false)}
          onSaved={() => {
            toast(t('parties.bank_saved'), 'ok');
            reload();
          }}
        />
      )}
      {mergeOpen && (
        <MergeDialog
          source={detail}
          open={mergeOpen}
          onClose={() => setMergeOpen(false)}
          onMerged={(survivor) => {
            toast(t('parties.merged_done'), 'ok');
            location.hash = `#/parties/${encodeURIComponent(survivor.id)}`;
          }}
        />
      )}
      {!party.vat_id && !party.iban && detail.bank_changes.length === 0 && (
        <Empty title={t('parties.sparse_title')}>{t('parties.sparse_body')}</Empty>
      )}
    </>
  );
}

function billStatusText(status: string): string {
  if (status === 'ready') return t('parties.bill_status_ready');
  if (status === 'posted') return t('parties.bill_status_posted');
  if (status === 'discarded') return t('parties.bill_status_discarded');
  return t('parties.bill_status_needs_review');
}

function taxKindKey(kind: string): 'parties.tax_kind_vat' | 'parties.tax_kind_tax' | 'parties.tax_kind_other' {
  if (kind === 'vat') return 'parties.tax_kind_vat';
  if (kind === 'tax') return 'parties.tax_kind_tax';
  return 'parties.tax_kind_other';
}

function kindKey(kind: string): 'parties.kind_billing' | 'parties.kind_shipping' | 'parties.kind_other' {
  if (kind === 'billing') return 'parties.kind_billing';
  if (kind === 'shipping') return 'parties.kind_shipping';
  return 'parties.kind_other';
}
