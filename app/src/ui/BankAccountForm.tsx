/**
 * Creating a bank account: name, IBAN (checked here for the common mistakes,
 * the server checks it again), the ledger account it links to (a chart account
 * tagged `bank`), the opening balance as a decimal, and an optional CSV mapping
 * set up from a sample export. A person creates accounts; an agent key is refused.
 */
import { useState } from 'preact/hooks';
import { bankAccountCreate, type CsvMapping } from '../api/bank';
import { useSession } from '../session';
import { accountDraftProblems, formatIban, normaliseIban, parseDecimalMinor, plainMajor } from '../lib/bank';
import { t, type MessageKey } from '../i18n';
import { ErrorBanner } from './common';
import { CsvWizard } from './BankCsvWizard';
import { accountOptions, bankLedgerAccounts, useChart } from './BankShared';
import { Select } from './Select';

const IBAN_KEYS: Record<'empty' | 'format' | 'checksum', MessageKey> = {
  empty: 'bank.iban_empty',
  format: 'bank.iban_format',
  checksum: 'bank.iban_checksum',
};

function today(): string {
  const now = new Date();
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
}

export function BankAccountForm() {
  const { client, toast } = useSession();
  const chart = useChart();
  const [name, setName] = useState('');
  const [iban, setIban] = useState('');
  const [bic, setBic] = useState('');
  const [currency, setCurrency] = useState('EUR');
  const [ledger, setLedger] = useState('');
  const [opening, setOpening] = useState('0.00');
  const [openingDate, setOpeningDate] = useState(today);
  const [touched, setTouched] = useState<Record<string, boolean>>({});
  const [csv, setCsv] = useState(false);
  const [sample, setSample] = useState<string | null>(null);
  const [sampleName, setSampleName] = useState('');
  const [mapping, setMapping] = useState<CsvMapping | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const [csvError, setCsvError] = useState<MessageKey | null>(null);

  const problems = accountDraftProblems({ name, iban, currency, ledgerAccount: ledger, opening, openingDate });
  const shown = (field: keyof typeof problems) => (touched[field] || touched.submit ? problems[field] : undefined);
  const ledgerChoices = accountOptions(bankLedgerAccounts(chart.data ?? []));
  const touch = (field: string) => setTouched((current) => ({ ...current, [field]: true }));
  const ibanKey = shown('iban') ? IBAN_KEYS[problems.iban ?? 'format'] : null;
  const hasProblems = Object.keys(problems).length > 0;

  const loadSample = async (event: Event) => {
    const input = event.target as HTMLInputElement;
    const file = input.files?.[0];
    if (!file) return;
    setSampleName(file.name);
    setSample(await file.text());
    setMapping(null);
  };

  const submit = async (event: Event) => {
    event.preventDefault();
    setTouched((current) => ({ ...current, submit: true, name: true, iban: true, currency: true, ledger: true, opening: true, openingDate: true }));
    setError(null);
    setCsvError(null);
    if (hasProblems) return;
    if (csv && sample !== null && mapping === null) {
      setCsvError('bank.csv_needs_mapping');
      return;
    }
    const minor = parseDecimalMinor(opening, currency);
    if (minor === null) return;
    setBusy(true);
    try {
      const account = await bankAccountCreate(client, {
        name: name.trim(),
        iban: normaliseIban(iban),
        bic: bic.trim() === '' ? null : bic.trim().toUpperCase(),
        currency,
        ledger_account_number: ledger,
        opening_amount: plainMajor(minor, currency),
        opening_date: openingDate,
        csv_mapping: csv ? mapping : null,
      });
      toast(t('bank.toast_created'));
      window.location.hash = `#/bank/${encodeURIComponent(account.id)}`;
    } catch (err) {
      setError(err);
    } finally {
      setBusy(false);
    }
  };

  return (
    <form class="bank-form" onSubmit={submit} noValidate aria-busy={busy ? 'true' : undefined}>
      <div class="field-row">
        <div class="field">
          <span id="bank-name-label">{t('bank.form_name')}</span>
          <input aria-labelledby="bank-name-label"
            id="bank-name"
            class="input"
            type="text"
            autoComplete="off"
            value={name}
            aria-invalid={shown('name') ? 'true' : undefined}
            aria-describedby={shown('name') ? 'bank-name-error' : undefined}
            onInput={(event) => setName((event.target as HTMLInputElement).value)}
            onBlur={() => touch('name')}
          />
          {shown('name') && (
            <span id="bank-name-error" class="field-error">
              {t('bank.form_required')}
            </span>
          )}
        </div>
        <div class="field">
          <span id="bank-iban-label">{t('bank.form_iban')}</span>
          <input aria-labelledby="bank-iban-label"
            id="bank-iban"
            class="input mono"
            type="text"
            inputMode="text"
            autoComplete="off"
            spellcheck={false}
            value={iban}
            aria-invalid={ibanKey ? 'true' : undefined}
            aria-describedby={ibanKey ? 'bank-iban-error' : 'bank-iban-hint'}
            onInput={(event) => setIban((event.target as HTMLInputElement).value)}
            onBlur={() => touch('iban')}
          />
          {ibanKey ? (
            <span id="bank-iban-error" class="field-error">
              {t(ibanKey)}
            </span>
          ) : (
            <span id="bank-iban-hint" class="hint">
              {iban.trim() === '' ? t('bank.form_iban_hint') : formatIban(iban)}
            </span>
          )}
        </div>
      </div>
      <div class="field-row">
        <div class="field">
          <span id="bank-bic-label">{t('bank.form_bic')}</span>
          <input aria-labelledby="bank-bic-label" id="bank-bic" class="input mono" type="text" autoComplete="off" value={bic} onInput={(event) => setBic((event.target as HTMLInputElement).value)} />
        </div>
        <div class="field">
          <span id="bank-currency-label">{t('bank.form_currency')}</span>
          <input aria-labelledby="bank-currency-label"
            id="bank-currency"
            class="input mono"
            type="text"
            maxLength={3}
            autoComplete="off"
            value={currency}
            aria-invalid={shown('currency') ? 'true' : undefined}
            aria-describedby={shown('currency') ? 'bank-currency-error' : 'bank-currency-hint'}
            onInput={(event) => setCurrency((event.target as HTMLInputElement).value.toUpperCase())}
            onBlur={() => touch('currency')}
          />
          {shown('currency') ? (
            <span id="bank-currency-error" class="field-error">
              {t('bank.form_currency_error')}
            </span>
          ) : (
            <span id="bank-currency-hint" class="hint">
              {t('bank.form_currency_hint')}
            </span>
          )}
        </div>
      </div>
      <div class="field">
        <span id="bank-ledger-label">{t('bank.form_ledger')}</span>
        {chart.error ? (
          <ErrorBanner error={chart.error} onRetry={chart.reload} />
        ) : chart.data && ledgerChoices.length === 0 ? (
          <p class="note">
            {t('bank.form_no_bank_tag')} <a href="#/accounts">{t('bank.form_go_accounts')}</a>
          </p>
        ) : (
          <Select
            id="bank-ledger"
            labelledBy="bank-ledger-label"
            value={ledger}
            placeholder={t('bank.form_ledger_choose')}
            emptyLabel={t('bank.form_ledger_choose')}
            invalid={shown('ledger') !== undefined}
            describedBy={shown('ledger') ? 'bank-ledger-error' : undefined}
            onBlur={() => touch('ledger')}
            options={ledgerChoices}
            onChange={setLedger}
          />
        )}
        {shown('ledger') && (
          <span id="bank-ledger-error" class="field-error">
            {t('bank.form_ledger_required')}
          </span>
        )}
        <span class="hint">{t('bank.form_ledger_hint')}</span>
      </div>
      <div class="field-row">
        <div class="field">
          <span id="bank-opening-label">{t('bank.form_opening')}</span>
          <input aria-labelledby="bank-opening-label"
            id="bank-opening"
            class="input num"
            type="text"
            inputMode="decimal"
            autoComplete="off"
            value={opening}
            aria-invalid={shown('opening') ? 'true' : undefined}
            aria-describedby={shown('opening') ? 'bank-opening-error' : 'bank-opening-hint'}
            onInput={(event) => setOpening((event.target as HTMLInputElement).value)}
            onBlur={() => touch('opening')}
          />
          {shown('opening') ? (
            <span id="bank-opening-error" class="field-error">
              {t('bank.form_opening_error')}
            </span>
          ) : (
            <span id="bank-opening-hint" class="hint">
              {t('bank.form_opening_hint')}
            </span>
          )}
        </div>
        <div class="field">
          <span id="bank-opening-date-label">{t('bank.form_opening_date')}</span>
          <input aria-labelledby="bank-opening-date-label"
            id="bank-opening-date"
            class="input"
            type="date"
            value={openingDate}
            aria-invalid={shown('openingDate') ? 'true' : undefined}
            onInput={(event) => setOpeningDate((event.target as HTMLInputElement).value)}
            onBlur={() => touch('openingDate')}
          />
        </div>
      </div>

      <fieldset class="field-group bank-csv-toggle">
        <legend class="caption">{t('bank.form_csv_section')}</legend>
        <label class="check">
          <input type="checkbox" checked={csv} onChange={(event) => setCsv((event.target as HTMLInputElement).checked)} />
          {t('bank.form_csv_toggle')}
        </label>
        {csv && (
          <div class="bank-csv-setup">
            <div class="field">
              <span id="bank-csv-sample-label">{t('bank.form_csv_sample')}</span>
              <input aria-labelledby="bank-csv-sample-label" id="bank-csv-sample" class="input" type="file" accept=".csv,.tsv,.txt,text/csv,text/plain" onChange={(event) => void loadSample(event)} />
              <span class="hint">{sampleName ? t('bank.form_csv_sample_loaded', { name: sampleName }) : t('bank.form_csv_sample_hint')}</span>
            </div>
            {sample !== null && (
              <CsvWizard
                idPrefix="bank-create-csv"
                sample={sample}
                currency={currency || 'EUR'}
                initial={mapping}
                saveLabel={t('bank.csv_keep_mapping')}
                onSave={async (next) => {
                  setMapping(next);
                  setCsvError(null);
                }}
              />
            )}
            {mapping && <p class="note">{t('bank.csv_mapping_kept')}</p>}
            {csvError && (
              <p class="field-error" role="alert">
                {t(csvError)}
              </p>
            )}
          </div>
        )}
      </fieldset>

      {error !== null && <ErrorBanner error={error} />}
      <div class="form-actions">
        <a class="btn btn-ghost" href="#/bank">
          {t('common.cancel')}
        </a>
        <button type="submit" class="btn btn-primary" disabled={busy} aria-busy={busy ? 'true' : undefined}>
          {busy ? t('common.working') : t('bank.form_submit')}
        </button>
      </div>
    </form>
  );
}
