import { useEffect, useState } from 'preact/hooks';
import { taxSettingsGet, taxSettingsUpdate, type TaxSettingsView } from '../../api/settings';
import { useAsync } from '../../hooks';
import { t } from '../../i18n';
import { useSession } from '../../session';
import { ErrorBanner } from '../common';
import { FormProblem, LoadingLine, SettingsPanel } from './common';

/** Settings → Tax: the small-business exemption, the VAT reporting basis, and the rates the platform carries for the company's country. */
export function TaxSettings() {
  const { client, toast } = useSession();
  const state = useAsync(() => taxSettingsGet(client), [client.baseUrl, client.apiKey]);
  const [form, setForm] = useState<{ small_business: boolean; vat_basis: 'accrual' | 'cash' } | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<unknown>(null);

  useEffect(() => {
    if (state.data) setForm({ small_business: state.data.small_business, vat_basis: state.data.vat_basis });
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
      const next = await taxSettingsUpdate(client, current);
      setForm({ small_business: next.small_business, vat_basis: next.vat_basis });
      toast(t('settings.tax_saved'));
      state.reload();
    } catch (err) {
      setError(err);
    } finally {
      setSaving(false);
    }
  }

  return (
    <>
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
                  <th scope="col">{t('settings.col_source')}</th>
                </tr>
              </thead>
              <tbody>
                {view.rates.map((rate) => (
                  <tr key={rate.category}>
                    <th scope="row">{t(rate.category === 'standard' ? 'settings.rate_standard' : 'settings.rate_reduced')}</th>
                    <td class="num">{rate.percent}%</td>
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
    </>
  );
}
