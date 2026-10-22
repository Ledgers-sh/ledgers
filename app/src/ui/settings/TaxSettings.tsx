import { useEffect, useState } from 'preact/hooks';
import {
  taxSettingsGet,
  taxSettingsUpdate,
  type TaxSettingsView,
  type UserRateInput,
} from '../../api/settings';
import { useAsync } from '../../hooks';
import { t, type MessageKey } from '../../i18n';
import { useSession } from '../../session';
import { ErrorBanner } from '../common';
import { FormProblem, LoadingLine, SettingsPanel } from './common';

const REGIME_KEY: Record<string, MessageKey> = {
  vat: 'settings.regime_vat',
  gst: 'settings.regime_gst',
  sales_tax: 'settings.regime_sales_tax',
  none: 'settings.regime_none',
};

const KIND_KEY: Record<string, MessageKey> = {
  output_vat: 'settings.kind_output_vat',
  input_vat: 'settings.kind_input_vat',
  zero: 'settings.kind_zero',
  exempt: 'settings.kind_exempt',
  reverse_charge: 'settings.kind_reverse_charge',
  import_vat: 'settings.kind_import_vat',
};

const CATEGORY_KEY: Record<string, MessageKey> = {
  standard: 'settings.rate_standard',
  reduced: 'settings.rate_reduced',
  zero: 'settings.rate_zero',
};

const EMPTY_RATE: UserRateInput = { category: 'standard', percent: '', valid_from: '', source: '' };

/**
 * Settings → Tax: the country's tax pack (regime, registration number rule,
 * rates with their dates, returns and tax codes), the company's own rates for
 * packs that allow them, and the small-business and VAT-basis settings.
 */
export function TaxSettings() {
  const { client, toast } = useSession();
  const state = useAsync(() => taxSettingsGet(client), [client.baseUrl, client.apiKey]);
  const [form, setForm] = useState<{ small_business: boolean; vat_basis: 'accrual' | 'cash' } | null>(null);
  const [userRates, setUserRates] = useState<UserRateInput[]>([]);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<unknown>(null);

  useEffect(() => {
    if (state.data) {
      setForm({ small_business: state.data.small_business, vat_basis: state.data.vat_basis });
      setUserRates(state.data.user_rates.map((rate) => ({ ...rate })));
    }
  }, [state.data]);

  if (state.error && !state.data) return <ErrorBanner error={state.error} onRetry={state.reload} />;
  if (!state.data || !form) return <LoadingLine />;
  const view: TaxSettingsView = state.data;
  const current = form;

  async function submit(event: Event) {
    event.preventDefault();
    setSaving(true);
    setError(null);
    try {
      const body = view.user_rates_allowed ? { ...current, user_rates: userRates } : current;
      const next = await taxSettingsUpdate(client, body);
      setForm({ small_business: next.small_business, vat_basis: next.vat_basis });
      toast(view.user_rates_allowed ? t('settings.user_rates_saved') : t('settings.tax_saved'));
      state.reload();
    } catch (err) {
      setError(err);
    } finally {
      setSaving(false);
    }
  }

  function updateRate(index: number, patch: Partial<UserRateInput>) {
    setUserRates(userRates.map((rate, i) => (i === index ? { ...rate, ...patch } : rate)));
  }

  return (
    <>
      <SettingsPanel id="tax-pack-title" title={t('settings.pack')}>
        {view.pack ? (
          <dl class="facts">
            <dt>{t('settings.pack')}</dt>
            <dd>{view.pack_name}</dd>
            <dt>{t('settings.regime')}</dt>
            <dd>{view.regime && REGIME_KEY[view.regime] ? t(REGIME_KEY[view.regime]) : view.regime}</dd>
            <dt>{t('settings.registration_number')}</dt>
            <dd>
              {view.registration_label}
              <span class="hint block">{view.registration_note}</span>
            </dd>
          </dl>
        ) : (
          <p class="muted">{t('settings.no_country')}</p>
        )}
        {view.rates_status === 'pending' ? <p class="hint">{t('settings.rates_pending')}</p> : null}
      </SettingsPanel>

      <SettingsPanel id="tax-rules-title" title={t('settings.tax_rules')}>
        <form class="settings-form" onSubmit={submit} aria-labelledby="tax-rules-title">
          <FormProblem error={error} />
          <label class="check-row">
            <input
              type="checkbox"
              name="small_business"
              checked={current.small_business}
              onChange={(event) => setForm({ ...current, small_business: (event.target as HTMLInputElement).checked })}
            />
            <span>
              <strong>{t('settings.small_business')}</strong>
              <span class="hint block">{t('settings.small_business_hint')}</span>
            </span>
          </label>
          <fieldset class="field-group">
            <legend>{t('settings.vat_basis')}</legend>
            {(['accrual', 'cash'] as const).map((basis) => (
              <label class="check-row" key={basis}>
                <input
                  type="radio"
                  name="vat_basis"
                  value={basis}
                  checked={current.vat_basis === basis}
                  onChange={() => setForm({ ...current, vat_basis: basis })}
                />
                <span>
                  <strong>{t(basis === 'accrual' ? 'settings.vat_basis_accrual' : 'settings.vat_basis_cash')}</strong>
                  <span class="hint block">{t(basis === 'accrual' ? 'settings.vat_basis_accrual_hint' : 'settings.vat_basis_cash_hint')}</span>
                </span>
              </label>
            ))}
          </fieldset>

          {view.user_rates_allowed ? (
            <fieldset class="field-group">
              <legend>{t('settings.user_rates')}</legend>
              <p class="hint">{t('settings.user_rates_hint')}</p>
              {userRates.map((rate, index) => (
                <div class="field-row" key={index}>
                  <label class="field">
                    <span>{t('settings.col_rate')}</span>
                    <select
                      class="input"
                      value={rate.category}
                      onChange={(event) => updateRate(index, { category: (event.target as HTMLSelectElement).value as UserRateInput['category'] })}
                    >
                      {(['standard', 'reduced', 'zero'] as const).map((category) => (
                        <option key={category} value={category}>
                          {t(CATEGORY_KEY[category])}
                        </option>
                      ))}
                    </select>
                  </label>
                  <label class="field">
                    <span>{t('settings.col_percent')}</span>
                    <input
                      class="input mono"
                      inputMode="decimal"
                      value={rate.percent}
                      onInput={(event) => updateRate(index, { percent: (event.target as HTMLInputElement).value })}
                    />
                  </label>
                  <label class="field">
                    <span>{t('settings.col_valid_from')}</span>
                    <input
                      class="input mono"
                      type="date"
                      value={rate.valid_from}
                      onInput={(event) => updateRate(index, { valid_from: (event.target as HTMLInputElement).value })}
                    />
                  </label>
                  <label class="field">
                    <span>{t('settings.col_source')}</span>
                    <input
                      class="input"
                      value={rate.source}
                      onInput={(event) => updateRate(index, { source: (event.target as HTMLInputElement).value })}
                    />
                  </label>
                  <button
                    type="button"
                    class="btn"
                    onClick={() => setUserRates(userRates.filter((_, i) => i !== index))}
                  >
                    {t('settings.user_rate_remove')}
                  </button>
                </div>
              ))}
              <button type="button" class="btn" onClick={() => setUserRates([...userRates, { ...EMPTY_RATE }])}>
                {t('settings.user_rate_add')}
              </button>
            </fieldset>
          ) : null}

          <div class="form-actions">
            <button type="submit" class="btn btn-primary" disabled={saving}>
              {t('settings.save')}
            </button>
          </div>
        </form>
      </SettingsPanel>

      <SettingsPanel id="tax-rates-title" title={t('settings.tax_rates')}>
        {view.rate_table ? (
          <div class="table-wrap">
            <table class="settings-table">
              <thead>
                <tr>
                  <th scope="col">{t('settings.col_rate')}</th>
                  <th scope="col" class="num">
                    {t('settings.col_percent')}
                  </th>
                  <th scope="col">{t('settings.col_valid_from')}</th>
                  <th scope="col">{t('settings.col_source')}</th>
                </tr>
              </thead>
              <tbody>
                {view.rates.map((rate) => (
                  <tr key={`${rate.category}-${rate.valid_from}`}>
                    <th scope="row">{t(CATEGORY_KEY[rate.category])}</th>
                    <td class="num">{rate.percent}%</td>
                    <td class="mono">{rate.valid_from}</td>
                    <td class="small">{rate.source}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : (
          <p class="muted">{view.country ? t('settings.no_rate_table', { country: view.country }) : t('settings.no_country')}</p>
        )}
        <p class="hint">{t('settings.rates_fixed')}</p>
      </SettingsPanel>

      {view.returns.length > 0 ? (
        <SettingsPanel id="tax-returns-title" title={t('settings.returns')}>
          <ul class="plain-list">
            {view.returns.map((ret) => (
              <li key={ret.id}>{ret.label}</li>
            ))}
          </ul>
          <p class="hint">{t('settings.returns_hint')}</p>
        </SettingsPanel>
      ) : null}

      {view.tax_codes.length > 0 ? (
        <SettingsPanel id="tax-codes-title" title={t('settings.tax_codes')}>
          <div class="table-wrap">
            <table class="settings-table">
              <thead>
                <tr>
                  <th scope="col">{t('settings.col_code')}</th>
                  <th scope="col">{t('settings.col_label')}</th>
                  <th scope="col">{t('settings.col_kind')}</th>
                </tr>
              </thead>
              <tbody>
                {view.tax_codes.map((code) => (
                  <tr key={code.code}>
                    <th scope="row" class="mono">{code.code}</th>
                    <td>{code.label}</td>
                    <td>{t(KIND_KEY[code.kind])}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </SettingsPanel>
      ) : null}
    </>
  );
}
