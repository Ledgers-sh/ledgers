/**
 * Create and edit a party. Each field is checked by the server's own validators
 * (parties.validate) when the person leaves it, and the whole form is checked
 * again on save; the server's message for a field is shown under that field.
 * Bank details are not on this form: they change on the party page, by a person.
 */
import { useEffect, useRef, useState } from 'preact/hooks';
import { partyCreate, partyGet, partyUpdate, partyValidate, type Party, type PartyDetail } from '../api/parties';
import { useAsync } from '../hooks';
import { t } from '../i18n';
import { useSession } from '../session';
import { ErrorBanner, PageHead, Skeleton } from './common';
import { FormProblem } from './settings/common';
import { Select } from './Select';
import { AddressEditor, ContactEditor, Field } from './PartyFields';
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
  const headingRef = useRef<HTMLHeadingElement>(null);

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
    if (!(await check())) return;
    setSaving(true);
    try {
      if (mode === 'create') {
        const made = await partyCreate(client, createInputFromDraft(draft));
        toast(t('parties.saved_created'), 'ok');
        location.hash = `#/parties/${encodeURIComponent(made.party.id)}`;
      } else if (party) {
        await partyUpdate(client, party.id, updateInputFromDraft(draft));
        toast(t('parties.saved_updated'), 'ok');
        location.hash = `#/parties/${encodeURIComponent(party.id)}`;
      }
    } catch (error) {
      setProblem(error);
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
        <FormProblem error={problem} />

        <fieldset class="party-group">
          <legend>{t('parties.group_basics')}</legend>
          <div class="party-grid">
            <div id="party-name-wrap" class="party-span">
              <Field id="party-name" label={t('parties.name')} value={draft.name} onInput={(v) => setField('name', v)} onBlur={check} error={errorFor('name')} required />
            </div>
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
            <Field id="party-terms" label={t('parties.payment_terms')} type="number" inputMode="numeric" value={draft.payment_terms_days} onInput={(v) => setField('payment_terms_days', v)} onBlur={check} error={errorFor('payment_terms_days')} hint={t('parties.payment_terms_hint')} />
            <Field id="party-currency" label={t('parties.currency')} value={draft.currency} onInput={(v) => setField('currency', v)} mono ltr hint={t('parties.currency_hint')} />
            <Field id="party-language" label={t('parties.language')} value={draft.language} onInput={(v) => setField('language', v)} ltr />
            <Field id="party-account" label={t('parties.default_account')} value={draft.default_account} onInput={(v) => setField('default_account', v)} mono ltr />
            <Field id="party-tax-code" label={t('parties.default_tax_code')} value={draft.default_tax_code} onInput={(v) => setField('default_tax_code', v)} mono ltr />
          </div>
          <label class="check-row party-check">
            <input type="checkbox" checked={draft.personal_data} onChange={(event) => setField('personal_data', (event.target as HTMLInputElement).checked)} />
            <span>{t('parties.personal_data')}</span>
          </label>
        </fieldset>

        <AddressEditor idPrefix="party" addresses={draft.addresses} onChange={(next) => setField('addresses', next)} error={errorFor('addresses')} disabled={saving} />
        <ContactEditor idPrefix="party" contacts={draft.contacts} onChange={(next) => setField('contacts', next)} error={errorFor('contacts')} disabled={saving} />

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
