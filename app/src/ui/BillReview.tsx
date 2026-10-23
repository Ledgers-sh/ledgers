import { useEffect, useMemo, useState } from 'preact/hooks';
import {
  absoluteUrl,
  billConfirm,
  billDiscard,
  billGet,
  billPost,
  billUpdate,
  documentGet,
  documentUrl,
  expenseAccounts,
  type AccountChoice,
  type Bill,
  type BillFields,
  type BillLine,
  type Document,
} from '../api/bills';
import { useAsync } from '../hooks';
import { useSession } from '../session';
import { parseMajorToMinor, toPlainMajor } from '../lib/money';
import { colourOf } from '../lib/units';
import { accountNameFromCatalogue } from '../i18n/chart';
import { ConfirmDialog, ErrorBanner, Money, PageHead, Skeleton, useToasts, Toasts } from './common';
import { Select } from './Select';
import { PartyPicker } from './PartyPicker';
import { ReadValue } from './PartyFields';
import { formatIban } from '../lib/parties';
import { t, useLocale, type MessageKey } from '../i18n';

/** Below this, the field is highlighted for a closer look. */
export const LOW_CONFIDENCE = 70;

/** The form's text state. Amounts are kept as typed, in major units. */
interface Draft {
  /** The business unit the expense lines belong to; empty when none is chosen. */
  unit: string;
  /** The party the bill is for, chosen with the party picker. */
  vendor_id: string;
  vendor_name: string;
  vendor_vat_id: string;
  vendor_iban: string;
  invoice_number: string;
  invoice_date: string;
  due_date: string;
  net: string;
  vat: string;
  gross: string;
  lines: DraftLine[];
}

interface DraftLine {
  description: string;
  net: string;
  rate: string;
  vat: string;
  gross: string;
  account: string;
  suggestedFrom: string | null;
}

function draftFrom(bill: Bill): Draft {
  const major = (minor: number | null) => (minor === null ? '' : toPlainMajor(String(minor), bill.currency));
  return {
    unit: bill.unit ?? '',
    vendor_id: bill.vendor.id,
    vendor_name: bill.vendor.name,
    vendor_vat_id: bill.vendor.vat_id ?? '',
    vendor_iban: bill.vendor.iban ?? '',
    invoice_number: bill.invoice_number ?? '',
    invoice_date: bill.invoice_date ?? '',
    due_date: bill.due_date ?? '',
    net: major(bill.net_minor),
    vat: major(bill.vat_minor),
    gross: major(bill.gross_minor),
    lines: bill.lines.map((line) => ({
      description: line.description,
      net: major(line.net_minor),
      rate: line.vat_rate_bp === null ? '' : toPlainMajor(String(line.vat_rate_bp), 'EUR'),
      vat: major(line.vat_minor),
      gross: major(line.gross_minor),
      account: line.expense_account ?? '',
      suggestedFrom: line.suggested_from,
    })),
  };
}

/** Parses an amount typed in major units; empty is absent, and garbage is an error. */
function minorOf(text: string, currency: string): { value: number | null; ok: boolean } {
  if (text.trim() === '') return { value: null, ok: true };
  const parsed = parseMajorToMinor(text, currency);
  if (parsed === null) return { value: null, ok: false };
  return { value: Number(parsed), ok: true };
}

/** A VAT rate typed as a percentage (19, 19.5) in basis points (1900, 1950). */
function rateOf(text: string): { value: number | null; ok: boolean } {
  if (text.trim() === '') return { value: null, ok: true };
  const parsed = parseMajorToMinor(text, 'EUR');
  return parsed === null ? { value: null, ok: false } : { value: Number(parsed), ok: true };
}

/** Builds the fields the server takes from the form; null when something will not parse. */
export function fieldsFrom(
  draft: Draft,
  currency: string,
  bill: Bill,
  unitsEnabled = false,
): { fields: BillFields } | { error: MessageKey } {
  // Once the company has units, a bill names the unit its expenses belong to (the server refuses to post without one).
  if (unitsEnabled && draft.unit === '') return { error: 'units.err_bill' };
  const amounts = [draft.net, draft.vat, draft.gross].map((text) => minorOf(text, currency));
  if (amounts.some((amount) => !amount.ok)) return { error: 'bills.err_amount' };
  const lines: BillLine[] = [];
  for (const line of draft.lines) {
    const net = minorOf(line.net, currency);
    const vat = minorOf(line.vat, currency);
    const gross = minorOf(line.gross, currency);
    const rate = rateOf(line.rate);
    if (!net.ok || !vat.ok || !gross.ok || !rate.ok) return { error: 'bills.err_amount' };
    lines.push({
      description: line.description,
      net_minor: net.value,
      vat_rate_bp: rate.value,
      vat_minor: vat.value,
      gross_minor: gross.value,
      expense_account: line.account === '' ? null : line.account,
      suggested_from: line.suggestedFrom,
    });
  }
  return {
    fields: {
      unit: draft.unit === '' ? null : draft.unit,
      vendor_id: draft.vendor_id === '' ? null : draft.vendor_id,
      vendor_name: draft.vendor_name.trim() === '' ? bill.vendor.name : draft.vendor_name.trim(),
      vendor_vat_id: draft.vendor_vat_id.trim() === '' ? null : draft.vendor_vat_id.trim(),
      vendor_iban: draft.vendor_iban.trim() === '' ? null : draft.vendor_iban.replace(/\s+/g, '').toUpperCase(),
      invoice_number: draft.invoice_number.trim() === '' ? null : draft.invoice_number.trim(),
      invoice_date: draft.invoice_date.trim() === '' ? null : draft.invoice_date.trim(),
      due_date: draft.due_date.trim() === '' ? null : draft.due_date.trim(),
      currency: bill.currency,
      net_minor: amounts[0].value,
      vat_minor: amounts[1].value,
      gross_minor: amounts[2].value,
      lines,
    },
  };
}

function isMine(bill: Bill, draft: Draft, currency: string): boolean {
  return JSON.stringify(draft) === JSON.stringify(draftFrom(bill)) && currency === bill.currency;
}

/** The review screen: the document beside the fields, the checks, the journal preview and the actions. */
export function BillReview({ billId }: { billId: string }) {
  const { client, toast, units } = useSession();
  const locale = useLocale();
  const unitsEnabled = units.length > 0;
  const bill = useAsync(() => billGet(client, billId).then((r) => r.bill), [client.baseUrl, client.apiKey, billId]);
  const accounts = useAsync(() => expenseAccounts(client), [client.baseUrl, client.apiKey]);
  const document = useAsync(
    () => (bill.data ? documentGet(client, bill.data.document_id).then((r) => r.document) : Promise.resolve(undefined)),
    [bill.data?.document_id, client.baseUrl],
  );
  const [draft, setDraft] = useState<Draft | null>(null);
  const [busy, setBusy] = useState<null | 'save' | 'post' | 'confirm' | 'discard'>(null);
  const [error, setError] = useState<unknown>(null);
  const [formError, setFormError] = useState<MessageKey | null>(null);
  const [discardOpen, setDiscardOpen] = useState(false);
  const [posted, setPosted] = useState<{ entryId: string; number: number } | null>(null);
  const { items, dismiss, push } = useToasts();

  useEffect(() => {
    if (bill.data) setDraft(draftFrom(bill.data));
  }, [bill.data?.id, bill.data?.updated_at]);

  if (bill.error !== undefined) return <ErrorBanner error={bill.error} onRetry={bill.reload} />;
  if (bill.data === undefined || draft === null) return <Skeleton rows={6} label={t('bills.loading')} />;
  const current = bill.data;
  const dirty = !isMine(current, draft, current.currency);
  const readOnly = current.status === 'posted' || current.status === 'discarded';
  const confidence = current.confidence;
  const chart = accounts.data ?? [];

  const update = (patch: Partial<Draft>) => setDraft((prev) => ({ ...(prev as Draft), ...patch }));
  const updateLine = (index: number, patch: Partial<DraftLine>) =>
    setDraft((prev) => {
      const base = prev as Draft;
      return { ...base, lines: base.lines.map((line, i) => (i === index ? { ...line, ...patch } : line)) };
    });

  const run = async (kind: 'save' | 'post' | 'confirm' | 'discard', work: () => Promise<void>) => {
    setBusy(kind);
    setError(null);
    try {
      await work();
    } catch (err) {
      setError(err);
    } finally {
      setBusy(null);
    }
  };

  const save = (event: Event) => {
    event.preventDefault();
    const built = fieldsFrom(draft, current.currency, current, unitsEnabled);
    if ('error' in built) {
      setFormError(built.error);
      return;
    }
    setFormError(null);
    void run('save', async () => {
      const updated = await billUpdate(client, current.id, built.fields);
      setDraft(draftFrom(updated.bill));
      bill.reload();
      push(t('bills.saved'), 'ok');
    });
  };

  const post = () => {
    if (unitsEnabled && draft.unit === '') {
      setFormError('units.err_bill');
      return;
    }
    void run('post', async () => {
      const result = await billPost(client, current.id);
      setPosted({ entryId: result.entry.entry_id, number: result.entry.number });
      bill.reload();
      toast(t('bills.posted'));
    });
  };

  const confirm = () =>
    run('confirm', async () => {
      await billConfirm(client, current.id);
      bill.reload();
      push(t('bills.confirmed'), 'ok');
    });

  const discard = () =>
    run('discard', async () => {
      await billDiscard(client, current.id);
      setDiscardOpen(false);
      toast(t('bills.discarded'));
      window.location.hash = '#/bills';
    });

  const checks = current.checks;
  const failed = checks.filter((check) => !check.ok);
  const canPost = current.status === 'ready' && !dirty && !readOnly;
  const needsVendor = current.vendor.needs_review || current.flags.includes('vendor-iban-changed');
  const alreadyPosted = current.status === 'posted';
  const preview = current.journal_preview;

  return (
    <>
      <PageHead title={t('bills.review_title')}>
        {current.vendor.name}
        {current.invoice_number ? ` · ${current.invoice_number}` : ''}
      </PageHead>
      <div class="bill-review">
        <DocumentPane document={document.data} config={client} />

        <form class="panel bill-form entry-form" onSubmit={save} noValidate aria-label={t('bills.fields_label')}>
          <div class="panel-head">
            <h2 class="grow">{t('bills.fields_heading')}</h2>
            <span class={`badge bill-status bill-status-${current.status}`}>{t(statusKey(current.status))}</span>
          </div>
          <div class="panel-body entry-form">
            {current.reader === 'unread-scan' && (
              <div class="banner human" role="status" data-testid="needs-photo">
                {t('bills.needs_photo')}
              </div>
            )}
            {current.status === 'needs-review' && <div class="banner human">{t('bills.needs_review_note')}</div>}
            <p class="muted reader-note" data-testid="reader">
              {t('bills.read_by', { reader: t(readerKey(current.reader)) })}
            </p>
            {posted && (
              <div class="banner" role="status">
                {t('bills.posted_as', { number: posted.number })}{' '}
                <a href={`#/journal/${encodeURIComponent(posted.entryId)}`}>{t('bills.open_entry')}</a>
              </div>
            )}
            {alreadyPosted && !posted && (
              <div class="banner" role="status">
                {t('bills.already_posted')}{' '}
                {current.posted_entry_id && <a href={`#/journal/${encodeURIComponent(current.posted_entry_id)}`}>{t('bills.open_entry')}</a>}
              </div>
            )}

            <fieldset class="field-group" disabled={readOnly}>
              <legend>{t('bills.group_vendor')}</legend>
              <PartyPicker
                id="bill-vendor"
                label={t('parties.bill_vendor')}
                selected={{ id: draft.vendor_id, name: draft.vendor_name, vat_id: draft.vendor_vat_id || null }}
                allowCreate
                disabled={readOnly}
                onPick={(party) =>
                  update({
                    vendor_id: party.id,
                    vendor_name: party.name,
                    vendor_vat_id: party.vat_id ?? '',
                    vendor_iban: party.iban ?? '',
                  })
                }
              />
              {confidence.vendor_name !== undefined && confidence.vendor_name < LOW_CONFIDENCE && (
                <span class="low-tag">{t('bills.low_confidence')}</span>
              )}
              <p class="muted party-bill-note">{t('parties.bill_vendor_note')}</p>
              <dl class="party-facts">
                <ReadValue label={t('bills.vat_id')} ltr>
                  {draft.vendor_vat_id ? <span class="mono">{draft.vendor_vat_id}</span> : <span class="muted">{t('parties.none')}</span>}
                </ReadValue>
                <ReadValue label={t('bills.iban')} ltr>
                  {draft.vendor_iban ? <span class="mono">{formatIban(draft.vendor_iban)}</span> : <span class="muted">{t('parties.none')}</span>}
                </ReadValue>
              </dl>
              {current.vendor.id !== draft.vendor_id && <span class="muted">{t('parties.bill_vendor_changed')}</span>}
              {needsVendor && (
                <div class="vendor-review">
                  <span>{t('bills.vendor_review_note')}</span>
                  {!readOnly && (
                    <button type="button" class="btn btn-sm" onClick={() => void confirm()} disabled={busy !== null}>
                      {t('bills.confirm_vendor')}
                    </button>
                  )}
                </div>
              )}
            </fieldset>

            {unitsEnabled && (
              <fieldset class="field-group" disabled={readOnly}>
                <legend>{t('units.group_bill')}</legend>
                <div class="field">
                  <span id="bill-unit-label">{t('units.bill_field')}</span>
                  <Select
                    id="bill-unit"
                    labelledBy="bill-unit-label"
                    value={draft.unit}
                    placeholder={t('units.choose_unit')}
                    invalid={formError === 'units.err_bill' && draft.unit === ''}
                    describedBy={formError === 'units.err_bill' && draft.unit === '' ? 'bill-unit-error' : 'bill-unit-hint'}
                    options={units.map((item) => ({ value: item.code, label: item.code, hint: item.name, swatch: colourOf(item) }))}
                    searchable={false}
                    onChange={(value) => update({ unit: value })}
                  />
                  {formError === 'units.err_bill' && draft.unit === '' ? (
                    <span id="bill-unit-error" class="field-error">{t('units.err_bill')}</span>
                  ) : (
                    <span id="bill-unit-hint" class="hint">{t('units.bill_hint')}</span>
                  )}
                </div>
              </fieldset>
            )}

            <fieldset class="field-group" disabled={readOnly}>
              <legend>{t('bills.group_invoice')}</legend>
              <TextField label={t('bills.invoice_number')} value={draft.invoice_number} mono onInput={(v) => update({ invoice_number: v })} confidence={confidence.invoice_number} />
              <div class="field-row">
                <TextField label={t('bills.invoice_date')} value={draft.invoice_date} placeholder="YYYY-MM-DD" mono onInput={(v) => update({ invoice_date: v })} confidence={confidence.invoice_date} />
                <TextField label={t('bills.due_date')} value={draft.due_date} placeholder="YYYY-MM-DD" mono onInput={(v) => update({ due_date: v })} confidence={confidence.due_date} />
              </div>
              <div class="field-row">
                <TextField label={`${t('bills.net')} (${current.currency})`} value={draft.net} mono num onInput={(v) => update({ net: v })} confidence={confidence.net_minor} />
                <TextField label={`${t('bills.vat')} (${current.currency})`} value={draft.vat} mono num onInput={(v) => update({ vat: v })} confidence={confidence.vat_minor} />
                <TextField label={`${t('bills.gross')} (${current.currency})`} value={draft.gross} mono num onInput={(v) => update({ gross: v })} confidence={confidence.gross_minor} />
              </div>
            </fieldset>

            <fieldset class="field-group" disabled={readOnly}>
              <legend>{t('bills.group_lines')}</legend>
              {draft.lines.length === 0 && <p class="muted">{t('bills.no_lines')}</p>}
              {draft.lines.map((line, index) => (
                <div class="line-card" key={index}>
                  <TextField label={t('bills.line_description')} value={line.description} onInput={(v) => updateLine(index, { description: v })} />
                  <div class="field-row">
                    <TextField label={`${t('bills.net')} (${current.currency})`} value={line.net} mono num onInput={(v) => updateLine(index, { net: v })} />
                    <TextField label={t('bills.rate')} value={line.rate} mono num onInput={(v) => updateLine(index, { rate: v })} />
                    <TextField label={`${t('bills.vat')} (${current.currency})`} value={line.vat} mono num onInput={(v) => updateLine(index, { vat: v })} />
                  </div>
                  <div class="field">
                    <span id={`bill-account-label-${index}`}>{t('bills.expense_account')}</span>
                    <Select
                      id={`bill-account-${index}`}
                      labelledBy={`bill-account-label-${index}`}
                      value={line.account}
                      placeholder={t('bills.choose_account')}
                      options={chart.map((choice) => ({
                        value: choice.number,
                        label: accountLabel(choice, locale.code),
                        hint: choice.number,
                        group: t(`accountType.${choice.type}` as MessageKey),
                      }))}
                      onChange={(value) => updateLine(index, { account: value, suggestedFrom: null })}
                    />
                    {line.suggestedFrom && (
                      <span class="hint">{t('bills.suggested_from', { bill: line.suggestedFrom })}</span>
                    )}
                  </div>
                  {!readOnly && (
                    <button type="button" class="btn btn-ghost btn-sm" onClick={() => setDraft((prev) => ({ ...(prev as Draft), lines: (prev as Draft).lines.filter((_, i) => i !== index) }))}>
                      {t('bills.remove_line')}
                    </button>
                  )}
                </div>
              ))}
              {!readOnly && (
                <button type="button" class="btn btn-sm" onClick={() => update({ lines: [...draft.lines, { description: '', net: '', rate: '19', vat: '', gross: '', account: '', suggestedFrom: null }] })}>
                  {t('bills.add_line')}
                </button>
              )}
            </fieldset>

            {formError && <p class="field-error" role="alert">{t(formError)}</p>}
            {error !== null && <ErrorBanner error={error} />}

            <div class="form-actions">
              {dirty && !readOnly && <span class="muted grow">{t('bills.unsaved')}</span>}
              {!readOnly && (
                <button type="button" class="btn btn-danger" onClick={() => setDiscardOpen(true)} disabled={busy !== null}>
                  {t('bills.discard')}
                </button>
              )}
              {!readOnly && (
                <button type="submit" class="btn" disabled={busy !== null || !dirty}>
                  {busy === 'save' ? t('bills.saving') : t('bills.save')}
                </button>
              )}
              {!readOnly && (
                <button type="button" class="btn btn-primary" onClick={() => void post()} disabled={!canPost || busy !== null}>
                  {busy === 'post' ? t('bills.posting') : t('bills.post')}
                </button>
              )}
            </div>
            {!readOnly && !canPost && <p class="muted">{postBlockedReason(current, dirty, t)}</p>}
          </div>
        </form>

        <aside class="panel bill-side" aria-label={t('bills.checks_heading')}>
          <div class="panel-head">
            <h2 class="grow">{t('bills.checks_heading')}</h2>
            <span class="muted num">{failed.length === 0 ? t('bills.checks_all_ok') : t('bills.checks_failed', { count: failed.length })}</span>
          </div>
          <ul class="check-list">
            {checks.map((check) => (
              <li key={check.id} class={check.ok ? 'bill-check ok' : 'bill-check fail'}>
                <span class="check-mark" aria-hidden="true">{check.ok ? '✓' : '✗'}</span>
                <span class="sr-only">{check.ok ? t('bills.check_passed') : t('bills.check_failed')}: </span>
                <span>{checkLabel(check.id, check.detail)}</span>
              </li>
            ))}
          </ul>
          {current.flags.length > 0 && (
            <div class="panel-body">
              <h3 class="label-sm">{t('bills.flags_heading')}</h3>
              <ul class="flag-list">
                {current.flags.map((flag) => (
                  <li key={flag}>{flagText(flag, t)}</li>
                ))}
              </ul>
            </div>
          )}
          <div class="panel-head">
            <h2 class="grow">{t('bills.preview_heading')}</h2>
          </div>
          <div class="table-wrap">
            <table class="table journal-preview">
              <caption class="sr-only">{t('bills.preview_caption')}</caption>
              <thead>
                <tr>
                  <th scope="col">{t('bills.col_account')}</th>
                  <th scope="col" class="num">{t('bills.col_debit')}</th>
                  <th scope="col" class="num">{t('bills.col_credit')}</th>
                </tr>
              </thead>
              <tbody>
                {preview.length === 0 && (
                  <tr>
                    <td colSpan={3} class="muted">{t('bills.preview_empty')}</td>
                  </tr>
                )}
                {preview.map((line, index) => (
                  <tr key={index}>
                    <td class="mono">
                      {line.account} <span class="muted">{accountLabelByNumber(chart, line.account, locale.code)}</span>
                    </td>
                    <td class="num">{line.side === 'debit' ? <Money minor={String(line.minor)} currency={current.currency} /> : ''}</td>
                    <td class="num">{line.side === 'credit' ? <Money minor={String(line.minor)} currency={current.currency} /> : ''}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </aside>
      </div>

      <ConfirmDialog
        open={discardOpen}
        onClose={() => setDiscardOpen(false)}
        title={t('bills.discard_title')}
        confirmLabel={t('bills.discard')}
        onConfirm={() => void discard()}
        busy={busy === 'discard'}
        danger
      >
        <p>{t('bills.discard_body')}</p>
      </ConfirmDialog>
      <Toasts items={items} dismiss={dismiss} />
    </>
  );
}

function statusKey(status: Bill['status']): MessageKey {
  switch (status) {
    case 'ready':
      return 'bills.status_ready';
    case 'posted':
      return 'bills.status_posted';
    case 'discarded':
      return 'bills.status_discarded';
    default:
      return 'bills.status_needs_review';
  }
}

/** A check's label in the reader's language; the server's own sentence only when the id is unknown. */
function checkLabel(id: string, serverDetail: string): string {
  const key = `bills.check.${id}` as MessageKey;
  return CHECK_IDS.includes(id) ? t(key) : serverDetail;
}

const CHECK_IDS = [
  'currency', 'invoice-date', 'due-date', 'totals-present', 'gross-equals-net-plus-vat', 'lines-present',
  'lines-net-sum', 'lines-vat-sum', 'vat-rate', 'line-gross', 'amounts-in-document',
];

/** The message key that names the reader of a bill, for the review screen. */
export function readerKey(reader: string | null): MessageKey {
  switch (reader) {
    case 'rules':
      return 'bills.reader_rules';
    case 'minimax':
      return 'bills.reader_minimax';
    case 'anthropic':
      return 'bills.reader_anthropic';
    case 'unread-scan':
      return 'bills.reader_none';
    default:
      return 'bills.reader_manual';
  }
}

function flagText(flag: string, tr: (key: MessageKey) => string): string {
  if (flag.startsWith('instruction-text')) return tr('bills.flag_instruction');
  if (flag === 'vendor-new') return tr('bills.flag_vendor_new');
  if (flag === 'vendor-iban-changed') return tr('bills.flag_iban_changed');
  if (flag === 'no-expense-account') return tr('bills.flag_no_account');
  return flag;
}

function postBlockedReason(bill: Bill, dirty: boolean, tr: (key: MessageKey) => string): string {
  if (dirty) return tr('bills.post_blocked_unsaved');
  if (bill.status === 'needs-review') return tr('bills.post_blocked_review');
  return '';
}

function accountLabel(choice: AccountChoice, locale: string): string {
  return accountNameFromCatalogue(choice.name, locale) ?? choice.name;
}

function accountLabelByNumber(chart: AccountChoice[], number: string, locale: string): string {
  const found = chart.find((choice) => choice.number === number);
  return found ? accountLabel(found, locale) : '';
}

/** A labelled input. Low extraction confidence is marked in text, not only colour. */
function TextField({
  label,
  value,
  onInput,
  mono,
  num,
  placeholder,
  confidence,
}: {
  label: string;
  value: string;
  onInput: (value: string) => void;
  mono?: boolean;
  num?: boolean;
  placeholder?: string;
  confidence?: number;
}) {
  const low = confidence !== undefined && confidence < LOW_CONFIDENCE;
  return (
    <label class={`field${low ? ' field-low' : ''}`}>
      <span>
        {label}
        {low && <span class="low-tag"> · {t('bills.low_confidence')}</span>}
      </span>
      <input
        class={`input${mono ? ' mono' : ''}${num ? ' num' : ''}`}
        value={value}
        placeholder={placeholder}
        onInput={(event) => onInput((event.target as HTMLInputElement).value)}
        autoComplete="off"
        spellcheck={false}
        inputMode={num ? 'decimal' : undefined}
      />
    </label>
  );
}

/** The original file: a PDF in the browser's viewer (zoomable), or an image with zoom. */
function DocumentPane({ document, config }: { document: Document | undefined; config: Parameters<typeof documentUrl>[0] }) {
  const [zoom, setZoom] = useState(100);
  const link = useAsync(
    () => (document ? documentUrl(config, document.id) : Promise.resolve(null)),
    [document?.id, config.baseUrl, config.apiKey],
  );
  const src = useMemo(() => (link.data ? absoluteUrl(config, link.data.url) : null), [link.data, config.baseUrl]);
  if (document === undefined) return <section class="panel doc-pane" aria-label={t('bills.document_label')}><Skeleton rows={3} /></section>;
  const isPdf = document.mime === 'application/pdf';
  return (
    <section class="panel doc-pane" aria-label={t('bills.document_label')}>
      <div class="panel-head">
        <h2 class="grow">{t('bills.document_label')}</h2>
        <div class="row-actions" role="group" aria-label={t('bills.zoom_label')}>
          <button type="button" class="btn btn-sm" onClick={() => setZoom((z) => Math.max(50, z - 25))} aria-label={t('bills.zoom_out')}>−</button>
          <span class="num" aria-live="polite">{zoom}%</span>
          <button type="button" class="btn btn-sm" onClick={() => setZoom((z) => Math.min(300, z + 25))} aria-label={t('bills.zoom_in')}>+</button>
          <button type="button" class="btn btn-sm btn-ghost" onClick={() => setZoom(100)}>{t('bills.zoom_fit')}</button>
        </div>
      </div>
      <div class="doc-viewport">
        {src === null && <Skeleton rows={4} label={t('bills.loading_document')} />}
        {src !== null && isPdf && (
          <iframe class="doc-frame" title={document.original_name} src={`${src}#zoom=${zoom}`} />
        )}
        {src !== null && !isPdf && (
          <div class="doc-image-wrap" style={{ width: `${zoom}%` }}>
            <img class="doc-image" src={src} alt={t('bills.document_alt', { name: document.original_name })} />
          </div>
        )}
      </div>
      {src !== null && (
        <p class="doc-fallback">
          <a href={src} target="_blank" rel="noopener">
            {t('bills.open_original')}
          </a>
        </p>
      )}
      {link.error !== undefined && <ErrorBanner error={link.error} />}
    </section>
  );
}
