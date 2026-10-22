import { useEffect, useState } from 'preact/hooks';
import { companyGet, companyUpdate, type CompanyProfile, type CompanyView } from '../../api/settings';
import { CreateCompanyDialog } from '../CreateCompany';
import { useAsync } from '../../hooks';
import { t, type MessageKey } from '../../i18n';
import { useSession } from '../../session';
import { countryProblem, emailProblem, vatProblem } from '../../lib/vat';
import { ErrorBanner } from '../common';
import { FieldProblem, FormProblem, LoadingLine, SettingsPanel } from './common';

type CompanyForm = CompanyProfile & { name: string };
type FieldName = keyof CompanyForm;

/** The form's text for a company read. */
function formOf(view: CompanyView): CompanyForm {
  return { name: view.name, ...view.profile };
}

const VAT_MESSAGE: Record<'country_needed' | 'prefix' | 'format', MessageKey> = {
  country_needed: 'settings.vat_country_needed',
  prefix: 'settings.vat_prefix',
  format: 'settings.vat_format',
};

/** The field-level messages the form shows before it sends anything. */
export function companyProblems(form: CompanyForm): Partial<Record<FieldName, MessageKey>> {
  const problems: Partial<Record<FieldName, MessageKey>> = {};
  if (form.name.trim() === '') problems.name = 'settings.name_required';
  if (countryProblem(form.country)) problems.country = 'settings.country_format';
  const vat = vatProblem(form.country.trim().toUpperCase(), form.vat_id);
  if (vat) problems.vat_id = VAT_MESSAGE[vat];
  if (emailProblem(form.contact_email)) problems.contact_email = 'settings.email_format';
  return problems;
}

type FieldOptions = Partial<{
  maxLength: number;
  type: string;
  hint: MessageKey;
  mono: boolean;
  autoComplete: string;
  required: boolean;
}>;

/** Settings → Company: the legal identity on invoices, and the base currency (read only). */
export function CompanySettings() {
  const { client, toast } = useSession();
  const state = useAsync(() => companyGet(client), [client.baseUrl, client.apiKey]);
  const [form, setForm] = useState<CompanyForm | null>(null);
  const [problems, setProblems] = useState<Partial<Record<FieldName, MessageKey>>>({});
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const [saved, setSaved] = useState(false);
  const [creating, setCreating] = useState(false);
  const { person } = useSession();

  useEffect(() => {
    if (state.data) setForm(formOf(state.data));
  }, [state.data]);

  if (state.error && !state.data) return <ErrorBanner error={state.error} onRetry={state.reload} />;
  if (!state.data || !form) return <LoadingLine />;
  const view = state.data;
  const current = form;

  const set = (field: FieldName) => (event: Event) => {
    setSaved(false);
    setForm({ ...current, [field]: (event.target as HTMLInputElement).value });
  };

  async function submit(event: Event) {
    event.preventDefault();
    setError(null);
    const found = companyProblems(current);
    setProblems(found);
    const first = Object.keys(found)[0] as FieldName | undefined;
    if (first) {
      document.getElementById(`company-${first}`)?.focus();
      return;
    }
    setSaving(true);
    try {
      const { name, ...profile } = current;
      const next = await companyUpdate(client, { name: name.trim(), profile });
      setForm(formOf(next));
      setSaved(true);
      toast(t('settings.company_saved'));
      state.reload();
    } catch (err) {
      setError(err);
    } finally {
      setSaving(false);
    }
  }

  const field = (name: FieldName, label: MessageKey, options: FieldOptions = {}) => {
    const message = problems[name] ? t(problems[name]!) : null;
    const hintId = options.hint ? `company-${name}-hint` : undefined;
    const errorId = `company-${name}-error`;
    return (
      <label class="field">
        <span>{t(label)}</span>
        <input
          id={`company-${name}`}
          aria-label={t(label)}
          class={`input${options.mono ? ' mono' : ''}`}
          name={name}
          type={options.type ?? 'text'}
          value={current[name]}
          required={options.required}
          maxLength={options.maxLength}
          autoComplete={options.autoComplete ?? 'off'}
          aria-invalid={message ? 'true' : undefined}
          aria-describedby={[hintId, message ? errorId : null].filter(Boolean).join(' ') || undefined}
          onInput={set(name)}
        />
        {options.hint ? (
          <span id={hintId} class="hint">
            {t(options.hint)}
          </span>
        ) : null}
        <FieldProblem id={errorId} message={message} />
      </label>
    );
  };

  return (
    <>
      <SettingsPanel id="company-profile-title" title={t('settings.company_profile')}>
        <form class="settings-form" onSubmit={submit} noValidate aria-labelledby="company-profile-title">
          <FormProblem error={error} />
          <div class="field-row">
            {field('name', 'settings.company_name', { required: true, maxLength: 120, autoComplete: 'organization' })}
            {field('legal_form', 'settings.legal_form', { maxLength: 80, hint: 'settings.legal_form_hint' })}
          </div>
          <div class="field-row">
            {field('vat_id', 'settings.vat_id', { mono: true, maxLength: 20, hint: 'settings.vat_id_hint' })}
            {field('registration_number', 'settings.registration_number', { maxLength: 60, hint: 'settings.registration_hint' })}
          </div>
          {field('address_line', 'settings.address_line', { maxLength: 200, autoComplete: 'street-address' })}
          <div class="field-row">
            {field('postal_code', 'settings.postal_code', { maxLength: 20, autoComplete: 'postal-code' })}
            {field('city', 'settings.city', { maxLength: 100, autoComplete: 'address-level2' })}
            {field('country', 'settings.country', { maxLength: 2, mono: true, hint: 'settings.country_hint', autoComplete: 'country' })}
          </div>
          {view.base_currency_locked ? <p class="hint">{t('settings.country_locked_hint')}</p> : null}
          {field('contact_email', 'settings.contact_email', { type: 'email', maxLength: 254, autoComplete: 'email', hint: 'settings.contact_email_hint' })}
          <div class="form-actions">
            {saved ? (
              <span class="muted" role="status">
                {t('settings.company_saved')}
              </span>
            ) : null}
            <button type="submit" class="btn btn-primary" disabled={saving}>
              {t('settings.save')}
            </button>
          </div>
        </form>
      </SettingsPanel>

      <SettingsPanel id="company-currency-title" title={t('settings.base_currency')}>
        <dl class="settings-facts">
          <dt>{t('settings.base_currency')}</dt>
          <dd class="mono">{view.base_currency}</dd>
          <dt>{t('settings.base_currency_state')}</dt>
          <dd>{view.base_currency_locked ? t('settings.base_currency_locked') : t('settings.base_currency_open')}</dd>
        </dl>
        <p class="hint">{t('settings.base_currency_why')}</p>
      </SettingsPanel>

      {person ? (
        <SettingsPanel id="company-create-title" title={t('companies.settings_title')}>
          <p class="hint">{t('companies.settings_body')}</p>
          <div class="form-actions">
            <button type="button" class="btn" onClick={() => setCreating(true)}>
              {t('companies.create_title')}
            </button>
          </div>
          <CreateCompanyDialog open={creating} onClose={() => setCreating(false)} />
        </SettingsPanel>
      ) : null}
    </>
  );
}
