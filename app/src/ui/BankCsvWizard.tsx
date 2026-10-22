/**
 * The CSV column wizard: a bank's export has no fixed layout, so the person
 * says what each column is and checks the first rows as they will be read.
 * The preview is computed here in the browser; the server reads the file
 * again with the saved mapping when the statement is imported.
 */
import { useMemo, useState } from 'preact/hooks';
import type { CsvMapping } from '../api/bank';
import { DATE_FORMATS, guessMapping, mappingProblem, previewCsv, type CsvPreviewRow } from '../lib/bank';
import { t, type MessageKey } from '../i18n';
import { ErrorBanner } from './common';
import { Select, type SelectOption } from './Select';
import { SignedAmount } from './BankShared';

const PROBLEM_KEYS: Record<NonNullable<ReturnType<typeof mappingProblem>>, MessageKey> = {
  delimiter: 'bank.csv_err_delimiter',
  decimal: 'bank.csv_err_decimal',
  'date-format': 'bank.csv_err_date_format',
  'booking-date': 'bank.csv_err_booking_date',
  'amount-columns': 'bank.csv_err_amount_columns',
};

/** The wizard. `onSave` receives the mapping in the server's shape; the caller stores it. */
export function CsvWizard({
  idPrefix,
  sample,
  currency,
  initial,
  saveLabel,
  onSave,
}: {
  idPrefix: string;
  sample: string;
  currency: string;
  initial: CsvMapping | null;
  saveLabel: string;
  onSave: (mapping: CsvMapping) => Promise<void>;
}) {
  const [mapping, setMapping] = useState<CsvMapping>(() => initial ?? guessMapping(sample));
  const [split, setSplit] = useState(() => !(initial ?? guessMapping(sample)).amount);
  const [problem, setProblem] = useState<MessageKey | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [busy, setBusy] = useState(false);

  const preview = useMemo(() => previewCsv(sample, mapping, currency, 5), [sample, mapping, currency]);
  const columnOptions: SelectOption[] = preview.columns.map((column) => ({
    value: column.value,
    label: `${column.value.slice(1)}. ${column.label}`,
  }));
  const optional: SelectOption = { value: '', label: t('bank.csv_none') };
  const withNone = [optional, ...columnOptions];
  const choose = t('bank.csv_choose');

  const update = <K extends keyof CsvMapping>(key: K, value: CsvMapping[K]) => {
    setProblem(null);
    setMapping((current) => ({ ...current, [key]: value }));
  };
  const optionalRef = (value: string) => (value === '' ? null : value);

  const save = async () => {
    const shape = mappingProblem({ ...mapping, amount: split ? null : mapping.amount, debit: split ? mapping.debit : null, credit: split ? mapping.credit : null });
    if (shape) {
      setProblem(PROBLEM_KEYS[shape]);
      return;
    }
    const ready: CsvMapping = {
      ...mapping,
      amount: split ? null : mapping.amount,
      debit: split ? mapping.debit : null,
      credit: split ? mapping.credit : null,
      value_date: mapping.value_date || null,
      currency: mapping.currency || null,
      counterparty_name: mapping.counterparty_name || null,
      counterparty_iban: mapping.counterparty_iban || null,
      bank_reference: mapping.bank_reference || null,
      remittance: mapping.remittance.filter((ref) => ref !== ''),
    };
    setBusy(true);
    setError(null);
    try {
      await onSave(ready);
    } catch (err) {
      setError(err);
    } finally {
      setBusy(false);
    }
  };

  const remittance = [0, 1, 2].map((index) => mapping.remittance[index] ?? '');
  const setRemittance = (index: number, value: string) => {
    const next = [...remittance];
    next[index] = value;
    update('remittance', next);
  };

  return (
    <div class="bank-wizard">
      <fieldset class="field-group">
        <legend class="caption">{t('bank.csv_layout')}</legend>
        <div class="field-row">
          <div class="field">
            <span id={`${idPrefix}-delim-label`}>{t('bank.csv_delimiter')}</span>
            <Select
              id={`${idPrefix}-delim`}
              labelledBy={`${idPrefix}-delim-label`}
              value={mapping.delimiter}
              searchable={false}
              onChange={(value) => update('delimiter', value)}
              options={[
                { value: ';', label: t('bank.csv_delim_semicolon') },
                { value: ',', label: t('bank.csv_delim_comma') },
                { value: '\t', label: t('bank.csv_delim_tab') },
              ]}
            />
          </div>
          <div class="field">
            <span id={`${idPrefix}-decimal-label`}>{t('bank.csv_decimal')}</span>
            <Select
              id={`${idPrefix}-decimal`}
              labelledBy={`${idPrefix}-decimal-label`}
              value={mapping.decimal}
              searchable={false}
              onChange={(value) => update('decimal', value)}
              options={[
                { value: ',', label: t('bank.csv_decimal_comma') },
                { value: '.', label: t('bank.csv_decimal_point') },
              ]}
            />
          </div>
          <div class="field">
            <span id={`${idPrefix}-date-label`}>{t('bank.csv_date_format')}</span>
            <Select
              id={`${idPrefix}-date`}
              labelledBy={`${idPrefix}-date-label`}
              value={mapping.date_format}
              searchable={false}
              onChange={(value) => update('date_format', value)}
              options={DATE_FORMATS.map((format) => ({ value: format, label: format }))}
            />
          </div>
          <label class="field">
            <span>{t('bank.csv_skip_rows')}</span>
            <input
              class="input num"
              type="number"
              min="0"
              max="50"
              value={String(mapping.skip_rows)}
              onInput={(event) => update('skip_rows', Math.min(50, Math.max(0, Number((event.target as HTMLInputElement).value) || 0)))}
            />
          </label>
        </div>
        <label class="check">
          <input type="checkbox" checked={mapping.has_header} onChange={(event) => update('has_header', (event.target as HTMLInputElement).checked)} />
          {t('bank.csv_has_header')}
        </label>
      </fieldset>

      <fieldset class="field-group">
        <legend class="caption">{t('bank.csv_columns')}</legend>
        <div class="field-row">
          <div class="field">
            <span id={`${idPrefix}-booking-label`}>{t('bank.csv_booking_date')}</span>
            <Select
              id={`${idPrefix}-booking`}
              labelledBy={`${idPrefix}-booking-label`}
              value={mapping.booking_date}
              placeholder={choose}
              emptyLabel={choose}
              options={withNone.slice(1)}
              onChange={(value) => update('booking_date', value)}
            />
          </div>
          <div class="field">
            <span id={`${idPrefix}-value-label`}>{t('bank.csv_value_date')}</span>
            <Select
              id={`${idPrefix}-value`}
              labelledBy={`${idPrefix}-value-label`}
              value={mapping.value_date ?? ''}
              options={withNone}
              onChange={(value) => update('value_date', optionalRef(value))}
            />
          </div>
        </div>

        <fieldset class="bank-amount-mode">
          <legend class="label-sm">{t('bank.csv_amount_mode')}</legend>
          <label class="check">
            <input type="radio" name={`${idPrefix}-mode`} checked={!split} onChange={() => setSplit(false)} />
            {t('bank.csv_amount_single')}
          </label>
          <label class="check">
            <input type="radio" name={`${idPrefix}-mode`} checked={split} onChange={() => setSplit(true)} />
            {t('bank.csv_amount_split')}
          </label>
        </fieldset>
        <div class="field-row">
          {!split ? (
            <div class="field">
              <span id={`${idPrefix}-amount-label`}>{t('bank.csv_amount')}</span>
              <Select
                id={`${idPrefix}-amount`}
                labelledBy={`${idPrefix}-amount-label`}
                value={mapping.amount ?? ''}
                placeholder={choose}
                emptyLabel={choose}
                options={withNone.slice(1)}
                onChange={(value) => update('amount', optionalRef(value))}
              />
            </div>
          ) : (
            <>
              <div class="field">
                <span id={`${idPrefix}-debit-label`}>{t('bank.csv_debit')}</span>
                <Select
                  id={`${idPrefix}-debit`}
                  labelledBy={`${idPrefix}-debit-label`}
                  value={mapping.debit ?? ''}
                  placeholder={choose}
                  emptyLabel={choose}
                  options={withNone.slice(1)}
                  onChange={(value) => update('debit', optionalRef(value))}
                />
              </div>
              <div class="field">
                <span id={`${idPrefix}-credit-label`}>{t('bank.csv_credit')}</span>
                <Select
                  id={`${idPrefix}-credit`}
                  labelledBy={`${idPrefix}-credit-label`}
                  value={mapping.credit ?? ''}
                  placeholder={choose}
                  emptyLabel={choose}
                  options={withNone.slice(1)}
                  onChange={(value) => update('credit', optionalRef(value))}
                />
              </div>
            </>
          )}
          <div class="field">
            <span id={`${idPrefix}-currency-label`}>{t('bank.csv_currency')}</span>
            <Select
              id={`${idPrefix}-currency`}
              labelledBy={`${idPrefix}-currency-label`}
              value={mapping.currency ?? ''}
              options={withNone}
              onChange={(value) => update('currency', optionalRef(value))}
            />
          </div>
        </div>
        <div class="field-row">
          <div class="field">
            <span id={`${idPrefix}-name-label`}>{t('bank.csv_counterparty_name')}</span>
            <Select
              id={`${idPrefix}-name`}
              labelledBy={`${idPrefix}-name-label`}
              value={mapping.counterparty_name ?? ''}
              options={withNone}
              onChange={(value) => update('counterparty_name', optionalRef(value))}
            />
          </div>
          <div class="field">
            <span id={`${idPrefix}-iban-label`}>{t('bank.csv_counterparty_iban')}</span>
            <Select
              id={`${idPrefix}-iban`}
              labelledBy={`${idPrefix}-iban-label`}
              value={mapping.counterparty_iban ?? ''}
              options={withNone}
              onChange={(value) => update('counterparty_iban', optionalRef(value))}
            />
          </div>
          <div class="field">
            <span id={`${idPrefix}-ref-label`}>{t('bank.csv_bank_reference')}</span>
            <Select
              id={`${idPrefix}-ref`}
              labelledBy={`${idPrefix}-ref-label`}
              value={mapping.bank_reference ?? ''}
              options={withNone}
              onChange={(value) => update('bank_reference', optionalRef(value))}
            />
          </div>
        </div>
        <div class="field-row">
          {remittance.map((ref, index) => (
            <div class="field" key={index}>
              <span id={`${idPrefix}-rem${index}-label`}>{t('bank.csv_remittance', { n: index + 1 })}</span>
              <Select
                id={`${idPrefix}-rem${index}`}
                labelledBy={`${idPrefix}-rem${index}-label`}
                value={ref}
                options={withNone}
                onChange={(value) => setRemittance(index, value)}
              />
            </div>
          ))}
        </div>
      </fieldset>

      <fieldset class="field-group">
        <legend class="caption">{t('bank.csv_preview')}</legend>
        <p class="hint">{t('bank.csv_preview_hint', { count: preview.rows.length, records: preview.records })}</p>
        <div class="table-wrap">
          <table class="ledger bank-preview" aria-describedby={`${idPrefix}-preview-hint`}>
            <caption class="sr-only">{t('bank.csv_preview')}</caption>
            <thead>
              <tr>
                <th scope="col">{t('bank.col_date')}</th>
                <th scope="col">{t('bank.col_counterparty')}</th>
                <th scope="col">{t('bank.col_remittance')}</th>
                <th scope="col" class="num">{t('bank.col_amount')}</th>
              </tr>
            </thead>
            <tbody>
              {preview.rows.length === 0 && (
                <tr>
                  <td colSpan={4} class="muted">{t('bank.csv_no_rows')}</td>
                </tr>
              )}
              {preview.rows.map((row) => (
                <PreviewRow key={row.line} row={row} />
              ))}
            </tbody>
          </table>
        </div>
      </fieldset>

      {problem && (
        <p class="field-error" role="alert">
          {t(problem)}
        </p>
      )}
      {error !== null && <ErrorBanner error={error} />}
      <div class="form-actions">
        <button type="button" class="btn btn-primary" onClick={() => void save()} disabled={busy} aria-busy={busy ? 'true' : undefined}>
          {busy ? t('common.working') : saveLabel}
        </button>
      </div>
    </div>
  );
}

function PreviewRow({ row }: { row: CsvPreviewRow }) {
  return (
    <tr>
      <td class="mono">
        {row.bookingDate ?? <span class="field-error">{t('bank.csv_problem_date')}</span>}
      </td>
      <td>{row.counterparty || '—'}</td>
      <td class="bank-remit-cell">{row.remittance || '—'}</td>
      <td class="num">
        {row.amountMinor === null ? (
          <span class="field-error">{t('bank.csv_problem_amount')}</span>
        ) : (
          <SignedAmount minor={row.amountMinor.toString()} currency={row.currency} />
        )}
      </td>
    </tr>
  );
}
