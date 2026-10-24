/**
 * Form fields for the customers and vendors screens: one labelled input with
 * its server message, and the repeatable address and contact editors.
 */
import type { ComponentChildren } from 'preact';
import { t, useLocale } from '../i18n';
import { Icon } from './icons';
import { COUNTRY_CODES, countryName, emptyAddress, emptyContact, emptyTaxId, type AddressDraft, type ContactDraft, type TaxIdDraft } from '../lib/parties';
import type { PartyTaxId } from '../api/parties';
import { Select } from './Select';
import { Ltr } from '../i18n/Ltr';

export function Field({
  id,
  label,
  value,
  onInput,
  onBlur,
  error,
  hint,
  mono,
  type = 'text',
  inputMode,
  autoComplete,
  required,
  disabled,
  ltr,
}: {
  id: string;
  label: string;
  value: string;
  onInput: (value: string) => void;
  onBlur?: () => void;
  error?: string;
  hint?: string;
  mono?: boolean;
  type?: 'text' | 'email' | 'number';
  inputMode?: 'text' | 'numeric' | 'email' | 'decimal';
  autoComplete?: string;
  required?: boolean;
  disabled?: boolean;
  /** Text that is a code or a name in Latin script, kept left to right inside an Arabic page. */
  ltr?: boolean;
}) {
  const errorId = `${id}-error`;
  const hintId = `${id}-hint`;
  const describedBy = [error ? errorId : null, hint ? hintId : null].filter(Boolean).join(' ') || undefined;
  return (
    <div class={`field party-field${error ? ' invalid' : ''}`}>
      <label for={id}>
        {label}
        {required ? <span class="required" aria-hidden="true"> *</span> : null}
      </label>
      <input
        id={id}
        name={id}
        class={`input${mono ? ' mono' : ''}${ltr ? ' ltr-text' : ''}`}
        type={type}
        inputMode={inputMode}
        autoComplete={autoComplete}
        value={value}
        required={required}
        disabled={disabled}
        dir={ltr ? 'ltr' : undefined}
        aria-invalid={error ? 'true' : undefined}
        aria-describedby={describedBy}
        onInput={(event) => onInput((event.target as HTMLInputElement).value)}
        onBlur={onBlur}
      />
      {hint && (
        <span id={hintId} class="field-hint">
          {hint}
        </span>
      )}
      {error && (
        <span id={errorId} class="field-error">
          {error}
        </span>
      )}
    </div>
  );
}

/** A list of addresses: each one a small group of fields, with add and remove. */
export function AddressEditor({
  idPrefix,
  addresses,
  onChange,
  error,
  disabled,
}: {
  idPrefix: string;
  addresses: AddressDraft[];
  onChange: (next: AddressDraft[]) => void;
  /** The server's message about the list as a whole (one address is refused). */
  error?: string;
  disabled?: boolean;
}) {
  useLocale();
  const update = (index: number, patch: Partial<AddressDraft>) => onChange(addresses.map((a, i) => (i === index ? { ...a, ...patch } : a)));
  const remove = (index: number) => onChange(addresses.filter((_, i) => i !== index));
  return (
    <fieldset class="party-group" disabled={disabled} aria-describedby={error ? `${idPrefix}-error` : undefined}>
      <legend>{t('parties.addresses')}</legend>
      {addresses.length === 0 && <p class="muted">{t('parties.no_addresses')}</p>}
      {addresses.map((address, index) => {
        const n = index + 1;
        return (
          <div class="party-item" key={`${idPrefix}-a-${index}`}>
            <div class="party-item-head">
              <span class="party-item-title">{t('parties.address_n', { n })}</span>
              <button type="button" class="btn btn-sm btn-ghost" onClick={() => remove(index)} aria-label={t('parties.remove_address_n', { n })}>
                {t('parties.remove')}
              </button>
            </div>
            <div class="party-grid">
              <div class="field party-field">
                <label for={`${idPrefix}-a${index}-kind`}>{t('parties.address_kind')}</label>
                <Select
                  id={`${idPrefix}-a${index}-kind`}
                  labelledBy={`${idPrefix}-a${index}-kind-label`}
                  value={address.kind}
                  onChange={(kind) => update(index, { kind })}
                  options={[
                    { value: 'billing', label: t('parties.kind_billing') },
                    { value: 'shipping', label: t('parties.kind_shipping') },
                    { value: 'other', label: t('parties.kind_other') },
                  ]}
                />
              </div>
              <Field id={`${idPrefix}-a${index}-line1`} label={t('parties.line1')} value={address.line1} onInput={(v) => update(index, { line1: v })} />
              <Field id={`${idPrefix}-a${index}-line2`} label={t('parties.line2')} value={address.line2} onInput={(v) => update(index, { line2: v })} />
              <Field id={`${idPrefix}-a${index}-postal`} label={t('parties.postal_code')} value={address.postal_code} onInput={(v) => update(index, { postal_code: v })} ltr />
              <Field id={`${idPrefix}-a${index}-city`} label={t('parties.city')} value={address.city} onInput={(v) => update(index, { city: v })} />
              <div class="field party-field">
                <label for={`${idPrefix}-a${index}-country`}>{t('parties.country')}</label>
                <CountrySelect id={`${idPrefix}-a${index}-country`} value={address.country} onChange={(country) => update(index, { country })} />
              </div>
            </div>
          </div>
        );
      })}
      <button type="button" class="btn btn-sm" onClick={() => onChange([...addresses, emptyAddress()])}>
        <Icon name="plus" /> {t('parties.add_address')}
      </button>
      {error && (
        <span id={`${idPrefix}-error`} class="field-error">
          {error}
        </span>
      )}
    </fieldset>
  );
}

/** A country as a code the server stores, shown with its name in the reader's language. */
function CountrySelect({ id, value, onChange }: { id: string; value: string; onChange: (code: string) => void }) {
  const locale = useLocale();
  const codes = value && !(COUNTRY_CODES as readonly string[]).includes(value) ? [value, ...COUNTRY_CODES] : [...COUNTRY_CODES];
  return (
    <Select
      id={id}
      labelledBy={`${id}-label`}
      value={value}
      placeholder={t('parties.choose_country')}
      onChange={onChange}
      searchable
      options={codes.map((code) => ({ value: code, label: `${countryName(code, locale.code)} (${code})` }))}
    />
  );
}

/** A list of contacts: name, email, phone and role, with add and remove. */
export function ContactEditor({
  idPrefix,
  contacts,
  onChange,
  error,
  disabled,
}: {
  idPrefix: string;
  contacts: ContactDraft[];
  onChange: (next: ContactDraft[]) => void;
  error?: string;
  disabled?: boolean;
}) {
  const update = (index: number, patch: Partial<ContactDraft>) => onChange(contacts.map((c, i) => (i === index ? { ...c, ...patch } : c)));
  const remove = (index: number) => onChange(contacts.filter((_, i) => i !== index));
  return (
    <fieldset class="party-group" disabled={disabled} aria-describedby={error ? `${idPrefix}-error` : undefined}>
      <legend>{t('parties.contacts')}</legend>
      {contacts.length === 0 && <p class="muted">{t('parties.no_contacts')}</p>}
      {contacts.map((contact, index) => {
        const n = index + 1;
        return (
          <div class="party-item" key={`${idPrefix}-c-${index}`}>
            <div class="party-item-head">
              <span class="party-item-title">{t('parties.contact_n', { n })}</span>
              <button type="button" class="btn btn-sm btn-ghost" onClick={() => remove(index)} aria-label={t('parties.remove_contact_n', { n })}>
                {t('parties.remove')}
              </button>
            </div>
            <div class="party-grid">
              <Field id={`${idPrefix}-c${index}-name`} label={t('parties.contact_name')} value={contact.name} onInput={(v) => update(index, { name: v })} />
              <Field id={`${idPrefix}-c${index}-role`} label={t('parties.contact_role')} value={contact.role} onInput={(v) => update(index, { role: v })} />
              <Field id={`${idPrefix}-c${index}-email`} label={t('parties.contact_email')} type="email" value={contact.email} onInput={(v) => update(index, { email: v })} ltr autoComplete="email" />
              <Field id={`${idPrefix}-c${index}-phone`} label={t('parties.contact_phone')} type="text" inputMode="text" value={contact.phone} onInput={(v) => update(index, { phone: v })} ltr autoComplete="tel" />
            </div>
            <label class="check-row party-check">
              <input
                id={`${idPrefix}-c${index}-primary`}
                type="checkbox"
                checked={contact.primary}
                disabled={disabled}
                onChange={(event) => update(index, { primary: (event.target as HTMLInputElement).checked })}
              />
              <span>{t('parties.contact_primary')}</span>
            </label>
          </div>
        );
      })}
      <button type="button" class="btn btn-sm" onClick={() => onChange([...contacts, emptyContact()])}>
        <Icon name="plus" /> {t('parties.add_contact')}
      </button>
      {error && (
        <span id={`${idPrefix}-error`} class="field-error">
          {error}
        </span>
      )}
    </fieldset>
  );
}

/** A label above read-only text, for values the form does not change. */
export function ReadValue({ label, children, ltr }: { label: string; children: ComponentChildren; ltr?: boolean }) {
  return (
    <div class="party-value">
      <dt>{label}</dt>
      <dd>{ltr ? <Ltr>{children}</Ltr> : children}</dd>
    </div>
  );
}

/** The further tax IDs: a country, a kind and a value per row, with add and remove. */
export function TaxIdEditor({
  idPrefix,
  taxIds,
  onChange,
  error,
  disabled,
}: {
  idPrefix: string;
  taxIds: TaxIdDraft[];
  onChange: (next: TaxIdDraft[]) => void;
  error?: string;
  disabled?: boolean;
}) {
  useLocale();
  const update = (index: number, patch: Partial<TaxIdDraft>) => onChange(taxIds.map((r, i) => (i === index ? { ...r, ...patch } : r)));
  const remove = (index: number) => onChange(taxIds.filter((_, i) => i !== index));
  return (
    <fieldset class="party-group" disabled={disabled} aria-describedby={error ? `${idPrefix}-error` : undefined}>
      <legend>{t('parties.tax_ids')}</legend>
      <p class="muted">{t('parties.tax_ids_intro')}</p>
      {taxIds.map((row, index) => {
        const n = index + 1;
        return (
          <div class="party-item" key={`${idPrefix}-t-${index}`}>
            <div class="party-item-head">
              <span class="party-item-title">{t('parties.tax_id_n', { n })}</span>
              <button type="button" class="btn btn-sm btn-ghost" onClick={() => remove(index)} aria-label={t('parties.remove_tax_id_n', { n })}>
                {t('parties.remove')}
              </button>
            </div>
            <div class="party-grid">
              <div class="field party-field">
                <label for={`${idPrefix}-t${index}-country`}>{t('parties.country')}</label>
                <CountrySelect id={`${idPrefix}-t${index}-country`} value={row.country} onChange={(country) => update(index, { country })} />
              </div>
              <div class="field party-field">
                <label id={`${idPrefix}-t${index}-kind-label`} for={`${idPrefix}-t${index}-kind`}>
                  {t('parties.tax_kind')}
                </label>
                <Select
                  id={`${idPrefix}-t${index}-kind`}
                  labelledBy={`${idPrefix}-t${index}-kind-label`}
                  value={row.kind}
                  onChange={(kind) => update(index, { kind: kind as PartyTaxId['kind'] })}
                  options={[
                    { value: 'vat', label: t('parties.tax_kind_vat') },
                    { value: 'tax', label: t('parties.tax_kind_tax') },
                    { value: 'other', label: t('parties.tax_kind_other') },
                  ]}
                />
              </div>
              <Field id={`${idPrefix}-t${index}-value`} label={t('parties.tax_value')} value={row.value} onInput={(v) => update(index, { value: v })} mono ltr />
            </div>
          </div>
        );
      })}
      <button type="button" class="btn btn-sm" onClick={() => onChange([...taxIds, emptyTaxId()])}>
        <Icon name="plus" /> {t('parties.add_tax_id')}
      </button>
      {error && (
        <span id={`${idPrefix}-error`} class="field-error">
          {error}
        </span>
      )}
    </fieldset>
  );
}
