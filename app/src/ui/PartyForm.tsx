/**
 * Create and edit a party. Each field is checked by the server's own validators
 * (parties.validate) when the person leaves it, and the whole form is checked
 * again on save; the server's message for a field is shown under that field.
 * Bank details are not on this form: they change on the party page, by a person.
 */
import { useEffect, useRef, useState } from 'preact/hooks';
import { partyCreate, partyGet, partySetLogo, partyUpdate, partyValidate, type Party, type PartyDetail } from '../api/parties';
import { useAsync } from '../hooks';
import { t, type MessageKey } from '../i18n';
import { useSession } from '../session';
import { ErrorBanner, PageHead, Skeleton } from './common';
import { FormProblem } from './settings/common';
import { Select } from './Select';
import { AddressEditor, ContactEditor, Field, TaxIdEditor } from './PartyFields';
import { LogoField } from './LogoField';
import { partyProblemKey } from '../lib/problems';
import {
  createInputFromDraft,
  draftFromParty,
  emptyDraft,
  fieldErrors,
  termsValue,
  updateInputFromDraft,
  validateInputFromDraft,
  type PartyDraft,
} from '../lib/parties';
import type { PartyRoles } from '../api/parties';

type Errors = Record<string, string>;

export function PartyNew() {
  return <PartyFormPage mode="create" />;
}

export function PartyEdit({ id }: { id: string }) {
  const { client } = useSession();
  const detail = useAsync<PartyDetail>(() => partyGet(client, id), [id]);
  if (detail.loading && !detail.data) return <Skeleton rows={6} label={t('parties.loading')} />;
  if (detail.error && !detail.data) return <ErrorBanner error={detail.error} onRetry={detail.reload} />;
  if (!detail.data) return null;
  return <PartyFormPage mode="edit" party={detail.data.party} />;
}

function PartyFormPage({ mode, party }: { mode: 'create' | 'edit'; party?: Party }) {
  const { client, toast } = useSession();
  const [draft, setDraft] = useState<PartyDraft>(() => (party ? draftFromParty(party) : emptyDraft()));
  const [errors, setErrors] = useState<Errors>({});
  const [saving, setSaving] = useState(false);
  const [problem, setProblem] = useState<unknown>(null);
  const [problemKey, setProblemKey] = useState<MessageKey | null>(null);
  const [logoDoc, setLogoDoc] = useState<string | null>(party?.logo_document_id ?? null);
  const [logoError, setLogoError] = useState<string | null>(null);
  const headingRef = useRef<HTMLHeadingElement>(null);

  /** Shows a refusal: the app's own words for a known problem, the server's otherwise. */
  const refuse = (error: unknown) => {
    const key = partyProblemKey(error);
    setProblemKey(key);
    setProblem(key ? null : error);
  };

  /** The logo: in edit mode it is saved as soon as it is picked; in create mode it is saved with the party. */
  const pickLogo = async (documentId: string | null) => {
    setLogoError(null);
    if (mode === 'create' || !party) {
      setLogoDoc(documentId);
      return;
    }
    try {
      const saved = await partySetLogo(client, party.id, documentId);
      setLogoDoc(saved.party.logo_document_id);
      toast(t('parties.logo_saved'), 'ok');
    } catch (error) {
      const key = partyProblemKey(error);
      setLogoError(key ? t(key) : t('parties.logo_failed'));
    }
  };

  useEffect(() => {
    headingRef.current?.focus();
  }, []);

  const setField = <K extends keyof PartyDraft>(key: K, value: PartyDraft[K]) => {
    setDraft((d) => ({ ...d, [key]: value }));
    setErrors((e) => {
      if (!(key in e)) return e;
      const next = { ...e };
      delete next[key];
      return next;
    });
  };

  /** Asks the server about the fields the person has filled in, and shows what it refuses. */
  const check = async (): Promise<boolean> => {
    const terms = termsValue(draft.payment_terms_days);
    const local: Errors = {};
    if (Number.isNaN(terms)) local.payment_terms_days = t('parties.err_terms_whole');
    try {
      const result = await partyValidate(client, validateInputFromDraft(draft, party?.id));
      const server = fieldErrors(result.errors);
      const merged = { ...server, ...local };
      setErrors(merged);
      return Object.keys(merged).length === 0;
    } catch (error) {
      setProblem(error);
      return false;
    }
  };

  const submit = async (event: Event) => {
    event.preventDefault();
    setProblem(null);
    setProblemKey(null);
    if (!(await check())) return;
    setSaving(true);
    try {
      if (mode === 'create') {
        const made = await partyCreate(client, createInputFromDraft(draft));
        if (logoDoc) await partySetLogo(client, made.party.id, logoDoc);
        toast(t('parties.saved_created'), 'ok');
        location.hash = `#/parties/${encodeURIComponent(made.party.id)}`;
      } else if (party) {
        await partyUpdate(client, party.id, updateInputFromDraft(draft));
        toast(t('parties.saved_updated'), 'ok');
        location.hash = `#/parties/${encodeURIComponent(party.id)}`;
      }
    } catch (error) {
      refuse(error);
    } finally {
      setSaving(false);
    }
  };

  const errorFor = (field: string) => errors[field];
  const heading = mode === 'create' ? t('parties.new') : t('parties.edit_title');
  const cancelHref = party ? `#/parties/${encodeURIComponent(party.id)}` : '#/parties';

  return (
    <>
      <PageHead title={heading}>{mode === 'create' ? t('parties.new_intro') : t('parties.edit_intro')}</PageHead>
      <nav class="back-link-row">
        <a class="back-link" href={cancelHref}>
          {t('parties.back')}
        </a>
      </nav>
      <form class="party-form" onSubmit={submit} noValidate aria-labelledby="party-form-title" aria-busy={saving}>
        <h2 id="party-form-title" tabIndex={-1} ref={headingRef} class="sr-only">
          {heading}
        </h2>
        {problemKey && <p class="field-error party-problem" role="alert">{t(problemKey)}</p>}
        <FormProblem error={problem} />

        <fieldset class="party-group">
          <legend>{t('parties.group_basics')}</legend>
          <div class="party-grid">
            <div id="party-name-wrap" class="party-span">
              <Field id="party-name" label={t('parties.legal_name')} value={draft.name} onInput={(v) => setField('name', v)} onBlur={check} error={errorFor('name')} required />
            </div>
            <Field id="party-trading" label={t('parties.trading_name')} value={draft.trading_name} onInput={(v) => setField('trading_name', v)} error={errorFor('trading_name')} />
            <div class="field party-field">
              <label id="party-roles-label" for="party-roles">
                {t('parties.roles')}
              </label>
              <Select
                id="party-roles"
                labelledBy="party-roles-label"
                value={draft.roles}
                onChange={(value) => setField('roles', value as PartyRoles)}
                options={[
                  { value: 'vendor', label: t('parties.role_vendor') },
                  { value: 'customer', label: t('parties.role_customer') },
                  { value: 'both', label: t('parties.role_both') },
                ]}
              />
            </div>
            <Field id="party-vat" label={t('parties.vat_id')} value={draft.vat_id} onInput={(v) => setField('vat_id', v)} onBlur={check} error={errorFor('vat_id')} hint={t('parties.vat_hint')} mono ltr />
            <Field id="party-tax" label={t('parties.tax_number')} value={draft.tax_number} onInput={(v) => setField('tax_number', v)} mono ltr />
            <Field id="party-registration" label={t('parties.registration_number')} value={draft.registration_number} onInput={(v) => setField('registration_number', v)} onBlur={check} error={errorFor('registration_number')} mono ltr />
            <Field id="party-website" label={t('parties.website')} type="text" value={draft.website} onInput={(v) => setField('website', v)} onBlur={check} error={errorFor('website')} ltr autoComplete="url" />
            <Field id="party-email" label={t('parties.email')} type="email" value={draft.email} onInput={(v) => setField('email', v)} onBlur={check} error={errorFor('email')} ltr autoComplete="email" />
            <Field id="party-phone" label={t('parties.phone')} type="text" inputMode="text" value={draft.phone} onInput={(v) => setField('phone', v)} onBlur={check} error={errorFor('phone')} ltr autoComplete="tel" />
            <Field id="party-terms" label={t('parties.payment_terms')} type="number" inputMode="numeric" value={draft.payment_terms_days} onInput={(v) => setField('payment_terms_days', v)} onBlur={check} error={errorFor('payment_terms_days')} hint={t('parties.payment_terms_hint')} />
            <Field id="party-currency" label={t('parties.currency')} value={draft.currency} onInput={(v) => setField('currency', v)} onBlur={check} error={errorFor('currency')} mono ltr hint={t('parties.currency_hint')} />
            <Field id="party-language" label={t('parties.language')} value={draft.language} onInput={(v) => setField('language', v)} onBlur={check} error={errorFor('language')} ltr />
            <Field id="party-account" label={t('parties.default_account')} value={draft.default_account} onInput={(v) => setField('default_account', v)} mono ltr />
            <Field id="party-tax-code" label={t('parties.default_tax_code')} value={draft.default_tax_code} onInput={(v) => setField('default_tax_code', v)} mono ltr />
            <Field id="party-unit" label={t('parties.default_unit')} value={draft.default_unit} onInput={(v) => setField('default_unit', v)} onBlur={check} error={errorFor('default_unit')} mono ltr />
          </div>
          <label class="check-row party-check">
            <input type="checkbox" checked={draft.personal_data} onChange={(event) => setField('personal_data', (event.target as HTMLInputElement).checked)} />
            <span>{t('parties.personal_data')}</span>
          </label>
        </fieldset>

        <fieldset class="party-group party-logo-group">
          <legend>{t('parties.logo')}</legend>
          <LogoField labelId="party-logo-label" name={draft.name || t('parties.new')} logo={logoDoc ? { documentId: logoDoc } : null} onChange={(id) => void pickLogo(id)} />
          {logoError && <span class="field-error">{logoError}</span>}
        </fieldset>

        <TaxIdEditor idPrefix="party" taxIds={draft.tax_ids} onChange={(next) => setField('tax_ids', next)} error={errorFor('tax_ids')} disabled={saving} />

        <AddressEditor idPrefix="party" addresses={draft.addresses} onChange={(next) => setField('addresses', next)} error={errorFor('addresses')} disabled={saving} />
        <ContactEditor idPrefix="party" contacts={draft.contacts} onChange={(next) => setField('contacts', next)} error={errorFor('contacts')} disabled={saving} />

        <fieldset class="party-group">
          <legend>{t('parties.notes')}</legend>
          <Field id="party-tags" label={t('parties.tags')} value={draft.tags} onInput={(v) => setField('tags', v)} onBlur={check} error={errorFor('tags')} hint={t('parties.tags_hint')} />
          <div class="field party-field party-notes">
            <label for="party-notes">{t('parties.notes')}</label>
            <textarea
              id="party-notes"
              class="input"
              rows={4}
              value={draft.notes}
              aria-invalid={errorFor('notes') ? 'true' : undefined}
              onInput={(event) => setField('notes', (event.target as HTMLTextAreaElement).value)}
              onBlur={check}
            />
            {errorFor('notes') && <span class="field-error">{errorFor('notes')}</span>}
          </div>
        </fieldset>

        <div class="form-actions">
          <a class="btn" href={cancelHref}>
            {t('parties.cancel')}
          </a>
          <button type="submit" class="btn btn-primary" disabled={saving}>
            {mode === 'create' ? t('parties.create') : t('parties.save')}
          </button>
        </div>
      </form>
    </>
  );
}
