/**
 * Creating or changing an intercompany agreement. Each side names its own
 * accounts from its own chart, filtered to the right types. The withholding
 * is shown from the payer's country pack, with a treaty override field and
 * the country notes as advisory text with their sources. The server checks
 * everything again and books the withholding it decides on.
 */
import { useEffect, useState } from 'preact/hooks';
import { accountsList } from '../../api/commands';
import { companyGet } from '../../api/settings';
import { unitsList } from '../../api/commands';
import {
  agreementCreate,
  agreementGet,
  agreementUpdate,
  type AccountsInput,
  type AgreementBasis,
  type AgreementKind,
  type AgreementView,
  type GroupView,
  type Side,
} from '../../api/groups';
import type { Account, Unit } from '../../api/types';
import { useAsync } from '../../hooks';
import { accountName, intlLocale, t, type MessageKey } from '../../i18n';
import { Ltr } from '../../i18n/Ltr';
import { parseMajorToMinor, toPlainMajor } from '../../lib/money';
import { activeUnits } from '../../lib/units';
import {
  accountsFor,
  canChangeSettings,
  expectedWhtBp,
  formProblem,
  formatBpPercent,
  parsePercentToBp,
  whtDisplay,
  type SideAccounts,
} from '../../lib/groups';
import { countryNote, packRoyaltyWhtBp, type CountrySource } from '../../lib/countryNotes';
import { ErrorBanner, PageHead, Skeleton } from '../common';
import { Select, type SelectOption } from '../Select';
import { useGroupContext, type GroupContext } from './context';
import { CompanyName, GroupTabs, RoleNotice } from './shared';

const KINDS: AgreementKind[] = ['licence', 'royalty', 'management_fee', 'loan'];
const KIND_KEY: Record<AgreementKind, MessageKey> = {
  licence: 'agreements.kind.licence',
  royalty: 'agreements.kind.royalty',
  management_fee: 'agreements.kind.management_fee',
  loan: 'agreements.kind.loan',
};
const BASES: AgreementBasis[] = ['pct_of_revenue_bp', 'fixed_monthly', 'cost_plus_bp'];
const BASIS_KEY: Record<AgreementBasis, MessageKey> = {
  pct_of_revenue_bp: 'groups.basis.pct_of_revenue',
  fixed_monthly: 'groups.basis.fixed_monthly',
  cost_plus_bp: 'groups.basis.cost_plus',
};

type Role = 'royalty' | 'ic' | 'wht' | 'bank';
const EMPTY_ACCOUNTS: SideAccounts = { royalty: '', ic: '', wht: '', bank: '', unit: '' };

function toInput(side: SideAccounts, unitsOn: boolean): AccountsInput {
  return {
    royalty_account: side.royalty,
    ic_account: side.ic,
    wht_account: side.wht === '' ? null : side.wht,
    bank_account: side.bank,
    unit: unitsOn && side.unit !== '' ? side.unit : null,
  };
}

interface Chart {
  accounts: Account[];
  units: Unit[];
  country: string;
  baseCurrency: string;
}

/** Loads one company's chart, units and country. Each read names the company, not the page's. */
function useChartOf(ctx: GroupContext, companyId: string) {
  return useAsync(async (): Promise<Chart | null> => {
    if (companyId === '') return null;
    const config = { ...ctx.client, company: companyId };
    const [chart, units, company] = await Promise.all([accountsList(config), unitsList(config), companyGet(config)]);
    return {
      accounts: chart.accounts,
      units: activeUnits(units.units),
      country: company.profile.country,
      baseCurrency: company.base_currency,
    };
  }, [ctx.client.baseUrl, companyId]);
}

export function AgreementForm({ agreementId }: { agreementId: string | null }) {
  const ctx = useGroupContext();
  const existing = useAsync(
    () => (agreementId === null ? Promise.resolve(null) : agreementGet(ctx.client, agreementId)),
    [ctx.client.baseUrl, agreementId],
  );
  if (agreementId !== null && existing.error !== undefined) {
    return (
      <>
        <PageHead title={t('agreements.form_edit_title')} />
        <ErrorBanner error={existing.error} onRetry={existing.reload} />
      </>
    );
  }
  if (ctx.groups === undefined || ctx.companies === undefined || (agreementId !== null && existing.data === undefined)) {
    return <Skeleton rows={4} label={t('agreements.loading')} />;
  }
  return (
    <AgreementEditor
      ctx={ctx}
      groups={ctx.groups}
      existing={existing.data?.agreement ?? null}
      key={agreementId ?? 'new'}
    />
  );
}

function AgreementEditor({
  ctx,
  groups,
  existing,
}: {
  ctx: GroupContext;
  groups: GroupView[];
  existing: AgreementView | null;
}) {
  const companies = ctx.companies ?? [];
  const ownSide: Side = existing?.side ?? 'licensor';
  const [groupId, setGroupId] = useState(existing?.group_id ?? groups[0]?.id ?? '');
  const group = groups.find((item) => item.id === groupId);
  const [licensorId, setLicensorId] = useState(
    existing ? (ownSide === 'licensor' ? ctx.currentCompany : (existing.counterparty_id ?? '')) : ctx.currentCompany,
  );
  const [licenseeId, setLicenseeId] = useState(
    existing ? (ownSide === 'licensee' ? ctx.currentCompany : (existing.counterparty_id ?? '')) : '',
  );
  const [kind, setKind] = useState<AgreementKind>(existing?.kind ?? 'royalty');
  const [basis, setBasis] = useState<AgreementBasis>(existing?.basis ?? 'pct_of_revenue_bp');
  const [ratePercent, setRatePercent] = useState(existing?.rate_bp != null ? formatBpPercent(existing.rate_bp, intlLocale()) : '5');
  const [fixedText, setFixedText] = useState(
    existing?.fixed_minor != null ? toPlainMajor(String(existing.fixed_minor), existing.currency) : '',
  );
  const [currency, setCurrency] = useState(existing?.currency ?? '');
  const [treatyPercent, setTreatyPercent] = useState(
    existing?.treaty_override_bp != null ? formatBpPercent(existing.treaty_override_bp, intlLocale()) : '',
  );
  const [notes, setNotes] = useState(existing?.notes ?? '');
  const [licensorAccounts, setLicensorAccounts] = useState<SideAccounts>(() => initialAccounts(existing, 'licensor'));
  const [licenseeAccounts, setLicenseeAccounts] = useState<SideAccounts>(() => initialAccounts(existing, 'licensee'));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const [problem, setProblem] = useState<MessageKey | null>(null);

  const licensorChart = useChartOf(ctx, licensorId);
  const licenseeChart = useChartOf(ctx, licenseeId);
  const licensorRole = companies.find((company) => company.id === licensorId)?.role ?? '';
  const licenseeRole = companies.find((company) => company.id === licenseeId)?.role ?? '';
  const licensorCo = companies.find((company) => company.id === licensorId);
  const licenseeCo = companies.find((company) => company.id === licenseeId);
  const licensorName = ctx.nameOf(licensorId);
  const licenseeName = ctx.nameOf(licenseeId);

  // The agreement currency is the base currency both companies share; it defaults to the licensor's.
  useEffect(() => {
    if (currency === '' && licensorCo) setCurrency(licensorCo.baseCurrency);
  }, [licensorCo?.baseCurrency]);

  const memberOptions: SelectOption[] = (group?.members ?? []).map((member) => {
    const role = companies.find((company) => company.id === member.company_id)?.role ?? '';
    return {
      value: member.company_id,
      label: member.company_name,
      disabled: !canChangeSettings(role),
      hint: canChangeSettings(role) ? undefined : t('agreements.owner_hint'),
    };
  });
  const allowed = (companyId: string) => {
    const role = companies.find((company) => company.id === companyId)?.role ?? '';
    return canChangeSettings(role);
  };
  // A company's chart is read per company; a side is only offered once its chart is read.
  const licensorUnitsRequired = (licensorChart.data?.units.length ?? 0) > 0;
  const licenseeUnitsRequired = (licenseeChart.data?.units.length ?? 0) > 0;

  const withholding = kind === 'licence' || kind === 'royalty';
  const treaty = treatyPercent.trim() === '' ? null : parsePercentToBp(treatyPercent);
  const packBp = packRoyaltyWhtBp(licenseeChart.data?.country ?? '');
  const sameCountry = Boolean(licenseeChart.data && licensorChart.data && licenseeChart.data.country === licensorChart.data.country);
  const expected = expectedWhtBp({
    licenseeCountry: licenseeChart.data?.country ?? '',
    licensorCountry: licensorChart.data?.country ?? '',
    packRateBp: packBp,
    treatyOverrideBp: treaty,
  });

  const save = async () => {
    const found = formProblem({
      groupId,
      licensorId,
      licenseeId,
      licensorRole,
      licenseeRole,
      licensorCurrency: licensorCo?.baseCurrency ?? '',
      licenseeCurrency: licenseeCo?.baseCurrency ?? '',
      kind,
      basis,
      ratePercent,
      fixedText,
      currency,
      treatyPercent,
      licensorAccounts,
      licenseeAccounts,
      licensorUnitsRequired,
      licenseeUnitsRequired,
    });
    setProblem(found);
    if (found) return;
    const rate = basis === 'fixed_monthly' ? null : parsePercentToBp(ratePercent);
    const fixed = basis === 'fixed_monthly' ? parseMajorToMinor(fixedText, currency) : null;
    const common = {
      kind,
      basis,
      rate_bp: rate,
      fixed_minor: fixed === null ? null : Number(fixed),
      currency,
      treaty_override_bp: withholding ? treaty : null,
      notes: notes.trim() === '' ? null : notes.trim(),
      licensor: toInput(licensorAccounts, licensorUnitsRequired),
      licensee: toInput(licenseeAccounts, licenseeUnitsRequired),
    };
    setBusy(true);
    setError(null);
    try {
      if (existing) {
        await agreementUpdate(ctx.client, existing.id, common);
        window.location.hash = `#/group/agreements/${encodeURIComponent(existing.id)}`;
      } else {
        const created = await agreementCreate(ctx.client, {
          ...common,
          group_id: groupId,
          licensor_company_id: licensorId,
          licensee_company_id: licenseeId,
        });
        window.location.hash = `#/group/agreements/${encodeURIComponent(created.licensor.id)}`;
      }
    } catch (caught) {
      setError(caught);
    } finally {
      setBusy(false);
    }
  };

  const title = existing ? t('agreements.form_edit_title') : t('agreements.form_new_title');
  return (
    <>
      <PageHead title={title} />
      <GroupTabs current="agreements" />
      <form
        class="agreement-form"
        aria-labelledby="agreement-form-title"
        onSubmit={(event) => {
          event.preventDefault();
          void save();
        }}
      >
        <h2 id="agreement-form-title" class="sr-only">
          {title}
        </h2>
        {existing && existing.counterparty_id === null && <RoleNotice>{t('agreements.edit_hidden_counterparty')}</RoleNotice>}
        <section class="panel form-section" aria-labelledby="agreement-parties-heading">
          <div class="panel-head">
            <h3 id="agreement-parties-heading">{t('agreements.parties')}</h3>
          </div>
          <div class="panel-body field-row">
            <div class="field">
              <span id="agreement-group-label">{t('groups.pick_group')}</span>
              <Select
                id="agreement-group"
                labelledBy="agreement-group-label"
                value={groupId}
                options={groups.map((item) => ({ value: item.id, label: item.name }))}
                disabled={existing !== null}
                onChange={(value) => {
                  setGroupId(value);
                  setLicensorId(ctx.currentCompany);
                  setLicenseeId('');
                }}
              />
            </div>
            <div class="field">
              <span id="agreement-licensor-label">{t('agreements.licensor')}</span>
              <Select
                id="agreement-licensor"
                labelledBy="agreement-licensor-label"
                value={licensorId}
                options={memberOptions}
                placeholder={t('agreements.choose_company')}
                emptyLabel={t('agreements.choose_company')}
                disabled={existing !== null}
                onChange={(value) => setLicensorId(value)}
              />
            </div>
            <div class="field">
              <span id="agreement-licensee-label">{t('agreements.licensee')}</span>
              <Select
                id="agreement-licensee"
                labelledBy="agreement-licensee-label"
                value={licenseeId}
                options={memberOptions}
                placeholder={t('agreements.choose_company')}
                emptyLabel={t('agreements.choose_company')}
                disabled={existing !== null}
                onChange={(value) => setLicenseeId(value)}
              />
            </div>
            {licensorId !== '' && licenseeId !== '' && (!allowed(licensorId) || !allowed(licenseeId)) && (
              <RoleNotice>{t('agreements.err.settings_both')}</RoleNotice>
            )}
          </div>
        </section>

        <section class="panel form-section" aria-labelledby="agreement-terms-heading">
          <div class="panel-head">
            <h3 id="agreement-terms-heading">{t('agreements.terms')}</h3>
          </div>
          <div class="panel-body field-row">
            <div class="field">
              <span id="agreement-kind-label">{t('agreements.col.kind')}</span>
              <Select
                id="agreement-kind"
                labelledBy="agreement-kind-label"
                value={kind}
                options={KINDS.map((item) => ({ value: item, label: t(KIND_KEY[item]) }))}
                onChange={(value) => setKind(value as AgreementKind)}
              />
            </div>
            <div class="field">
              <span id="agreement-basis-label">{t('agreements.col.basis')}</span>
              <Select
                id="agreement-basis"
                labelledBy="agreement-basis-label"
                value={basis}
                options={BASES.map((item) => ({ value: item, label: t(BASIS_KEY[item]) }))}
                onChange={(value) => setBasis(value as AgreementBasis)}
              />
            </div>
            {basis === 'fixed_monthly' ? (
              <div class="field">
                <label for="agreement-fixed">{t('agreements.fixed_amount')}</label>
                <input
                  id="agreement-fixed"
                  class="input num"
                  inputMode="decimal"
                  value={fixedText}
                  onInput={(event) => setFixedText((event.target as HTMLInputElement).value)}
                />
              </div>
            ) : (
              <div class="field">
                <label for="agreement-rate">{t('agreements.rate_percent')}</label>
                <input
                  id="agreement-rate"
                  class="input num"
                  inputMode="decimal"
                  value={ratePercent}
                  onInput={(event) => setRatePercent((event.target as HTMLInputElement).value)}
                />
                <span class="hint">{t(basis === 'cost_plus_bp' ? 'agreements.rate_hint_cost' : 'agreements.rate_hint_revenue')}</span>
              </div>
            )}
            <div class="field">
              <span id="agreement-currency-label">{t('agreements.currency')}</span>
              <Select
                id="agreement-currency"
                labelledBy="agreement-currency-label"
                value={currency}
                options={[...new Set([licensorCo?.baseCurrency, licenseeCo?.baseCurrency].filter(Boolean) as string[])].map((code) => ({ value: code, label: code }))}
                placeholder={t('agreements.choose_company')}
                emptyLabel={t('agreements.choose_company')}
                onChange={(value) => setCurrency(value)}
              />
              <span class="hint">{t('agreements.currency_hint')}</span>
            </div>
            <div class="field notes-field">
              <label for="agreement-notes">{t('agreements.notes')}</label>
              <textarea id="agreement-notes" class="input" rows={2} value={notes} onInput={(event) => setNotes((event.target as HTMLTextAreaElement).value)} />
            </div>
          </div>
        </section>

        <section class="panel form-section" aria-labelledby="agreement-accounts-heading">
          <div class="panel-head">
            <h3 id="agreement-accounts-heading">{t('agreements.accounts')}</h3>
          </div>
          <div class="panel-body accounts-grid">
            <SideAccountsFields
              idPrefix="licensor"
              side="licensor"
              companyName={licensorName}
              chart={licensorChart.data}
              loading={licensorId !== '' && licensorChart.loading}
              accounts={licensorAccounts}
              onChange={setLicensorAccounts}
              withWht={withholding}
              unitsRequired={licensorUnitsRequired}
            />
            <SideAccountsFields
              idPrefix="licensee"
              side="licensee"
              companyName={licenseeName}
              chart={licenseeChart.data}
              loading={licenseeId !== '' && licenseeChart.loading}
              accounts={licenseeAccounts}
              onChange={setLicenseeAccounts}
              withWht={withholding}
              unitsRequired={licenseeUnitsRequired}
            />
          </div>
        </section>

        {withholding && (
          <section class="panel form-section wht-panel" aria-labelledby="agreement-wht-heading">
            <div class="panel-head">
              <h3 id="agreement-wht-heading">{t('groups.wht.label')}</h3>
            </div>
            <div class="panel-body">
              <div class="field">
                <label for="agreement-treaty">{t('agreements.treaty_override')}</label>
                <input
                  id="agreement-treaty"
                  class="input num"
                  inputMode="decimal"
                  value={treatyPercent}
                  onInput={(event) => setTreatyPercent((event.target as HTMLInputElement).value)}
                  aria-describedby="agreement-treaty-hint"
                />
                <span id="agreement-treaty-hint" class="hint">
                  {t('agreements.treaty_hint')}
                </span>
              </div>
              <WhtSummary
                sameCountry={sameCountry}
                expectedBp={expected.bp}
                basis={expected.basis}
                treatyBp={treaty}
                packBp={packBp}
                payerCountry={licenseeChart.data?.country ?? ''}
                loaded={Boolean(licenseeChart.data && licensorChart.data)}
              />
            </div>
          </section>
        )}

        <CountryNotes
          countries={[
            { label: licenseeName, iso: licenseeChart.data?.country ?? '' },
            { label: licensorName, iso: licensorChart.data?.country ?? '' },
          ]}
        />

        {problem && (
          <p class="field-error" role="alert">
            {t(problem)}
          </p>
        )}
        {error !== null && <ErrorBanner error={error} />}
        <div class="row-actions">
          <button type="submit" class="btn btn-primary" disabled={busy}>
            {existing ? t('agreements.save') : t('agreements.create')}
          </button>
          <a class="btn" href={existing ? `#/group/agreements/${encodeURIComponent(existing.id)}` : '#/group/agreements'}>
            {t('groups.cancel')}
          </a>
        </div>
      </form>
    </>
  );
}

function initialAccounts(existing: AgreementView | null, side: Side): SideAccounts {
  // Only the viewer's own side is known; the other company's accounts are picked again.
  if (!existing || existing.side !== side) return { ...EMPTY_ACCOUNTS };
  return {
    royalty: existing.accounts.royalty,
    ic: existing.accounts.ic,
    wht: existing.accounts.wht ?? '',
    bank: existing.accounts.bank,
    unit: existing.accounts.unit ?? '',
  };
}

function SideAccountsFields({
  idPrefix,
  side,
  companyName,
  chart,
  loading,
  accounts,
  onChange,
  withWht,
  unitsRequired,
}: {
  idPrefix: string;
  side: Side;
  companyName: string;
  chart: Chart | null | undefined;
  loading: boolean;
  accounts: SideAccounts;
  onChange: (next: SideAccounts) => void;
  withWht: boolean;
  unitsRequired: boolean;
}) {
  const options = (role: Role): SelectOption[] => {
    if (!chart) return [];
    return accountsFor(chart.accounts, role, side).map((account) => ({
      value: account.number,
      label: `${account.number} ${account.display_name ?? accountName(account.name_key)}`,
    }));
  };
  const field = (role: Role, labelKey: MessageKey) => (
    <div class="field">
      <span id={`${idPrefix}-${role}-label`}>{t(labelKey)}</span>
      <Select
        id={`${idPrefix}-${role}`}
        labelledBy={`${idPrefix}-${role}-label`}
        value={accounts[role]}
        options={options(role)}
        placeholder={loading ? t('agreements.loading_chart') : t('agreements.choose_account')}
        emptyLabel={loading ? t('agreements.loading_chart') : t('agreements.choose_account')}
        disabled={!chart}
        onChange={(value) => onChange({ ...accounts, [role]: value })}
      />
    </div>
  );
  return (
    <fieldset class="field-group side-accounts">
      <legend>
        {t(side === 'licensor' ? 'agreements.licensor_accounts' : 'agreements.licensee_accounts')}
        {' · '}
        <CompanyName name={companyName} />
      </legend>
      {field('royalty', side === 'licensor' ? 'agreements.acc.royalty_income' : 'agreements.acc.royalty_expense')}
      {field('ic', side === 'licensor' ? 'agreements.acc.ic_receivable' : 'agreements.acc.ic_payable')}
      {withWht && field('wht', side === 'licensor' ? 'agreements.acc.wht_receivable' : 'agreements.acc.wht_payable')}
      {field('bank', 'agreements.acc.bank')}
      {chart && chart.units.length > 0 && (
        <div class="field">
          <span id={`${idPrefix}-unit-label`}>{t('agreements.acc.unit')}</span>
          <Select
            id={`${idPrefix}-unit`}
            labelledBy={`${idPrefix}-unit-label`}
            value={accounts.unit}
            options={chart.units.map((unit) => ({ value: unit.code, label: unit.name }))}
            placeholder={t('agreements.choose_unit')}
            emptyLabel={t('agreements.choose_unit')}
            onChange={(value) => onChange({ ...accounts, unit: value })}
          />
          {unitsRequired && <span class="hint">{t('agreements.unit_required')}</span>}
        </div>
      )}
    </fieldset>
  );
}

function WhtSummary({
  sameCountry,
  expectedBp,
  basis,
  treatyBp,
  packBp,
  payerCountry,
  loaded,
}: {
  sameCountry: boolean;
  expectedBp: number;
  basis: ReturnType<typeof expectedWhtBp>['basis'];
  treatyBp: number | null;
  packBp: number | null;
  payerCountry: string;
  loaded: boolean;
}) {
  if (!loaded) return <p class="muted">{t('agreements.loading_chart')}</p>;
  const display = whtDisplay({ wht_bp: expectedBp, wht_basis: basis, treaty_override_bp: treatyBp });
  const source = countryNote(payerCountry)?.whtSource;
  return (
    <dl class="wht-summary">
      <div>
        <dt>{t('groups.wht.pack_rate')}</dt>
        <dd>
          {packBp === null ? t('groups.wht.no_pack_rate') : `${formatBpPercent(packBp, intlLocale())}%`}
          {source && <SourceLink source={source} />}
        </dd>
      </div>
      <div>
        <dt>{t('groups.wht.applied')}</dt>
        <dd>
          <strong class="num">{`${formatBpPercent(expectedBp, intlLocale())}%`}</strong> <span>{t(display.key)}</span>
        </dd>
      </div>
      {sameCountry && (
        <div>
          <dd>
            <span class="muted">{t('groups.wht.resident')}</span>
          </dd>
        </div>
      )}
    </dl>
  );
}

function SourceLink({ source }: { source: CountrySource }) {
  return (
    <a class="source-link" href={source.url} target="_blank" rel="noopener noreferrer">
      {source.label}
      <span class="sr-only"> ({t('agreements.opens_new_tab')})</span>
    </a>
  );
}

/** The advisory country notes for the two countries, each with its source. Not tax advice. */
function CountryNotes({ countries }: { countries: { label: string; iso: string }[] }) {
  const items = countries.flatMap((country) => {
    const note = countryNote(country.iso);
    if (!note) return [];
    return note.notes.map((entry) => ({ key: `${country.iso}-${entry.topic}`, country: country.label, iso: note.iso, topic: entry.topic, source: entry.source }));
  });
  if (items.length === 0) return null;
  return (
    <section class="panel form-section country-notes" aria-labelledby="country-notes-heading">
      <div class="panel-head">
        <h3 id="country-notes-heading">{t('agreements.country_notes')}</h3>
      </div>
      <div class="panel-body">
        <p class="muted">{t('agreements.country_notes_advisory')}</p>
        <ul class="country-note-list">
          {items.map((item) => (
            <li key={item.key}>
              <strong>
                <Ltr>{item.iso}</Ltr>
              </strong>{' '}
              <span>{t(`groups.country.${item.iso}.${item.topic}` as MessageKey)}</span>{' '}
              <SourceLink source={item.source} />
            </li>
          ))}
        </ul>
      </div>
    </section>
  );
}
