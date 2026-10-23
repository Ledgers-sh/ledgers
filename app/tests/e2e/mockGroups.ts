/**
 * The group routes of the fake API (see mockApi.ts): groups, intercompany
 * agreements, their periods, the reconciliation and the group view. The
 * companies are invented: Factory Zero IP Pte. Ltd. (Singapore) licenses a
 * royalty to PT Operasional Nusantara (Indonesia, 20% withholding under the
 * pack) and to Halden Works GmbH (Germany, no pack rate). A company's header
 * (X-Ledgers-Company) says which side the call reads. A failed post is injected
 * by the test through `groupsMock.failNextPost`; the rest follows the server's
 * rules (both sides post or neither, a failed side fails both, retry starts
 * again).
 */
import { readFileSync } from 'node:fs';
import { companyState, type MockCompany } from './mockCompanies';

/**
 * The logos of the fake companies, groups and units: document id to the PNG the
 * fake serves (the Factory Zero marks in fixtures/demo/logos). A logo has no
 * document in the bills fake, so these are served here.
 */
const LOGO_FILES: Record<string, string> = {
  DOCFZIP: 'fz-favicon.png',
  DOCFZ: 'fz-favicon.png',
  DOCLDG: 'ledgers.png',
};

/** The served bytes of a logo document, for GET /v1/documents/{id}/file. */
export function logoFile(path: string): { mime: string; bytes: Buffer } | null {
  const match = /^\/v1\/documents\/([^/]+)\/file/.exec(path);
  const name = match ? LOGO_FILES[decodeURIComponent(match[1] ?? '')] : undefined;
  if (!name) return null;
  return { mime: 'image/png', bytes: readFileSync(new URL(`../../../fixtures/demo/logos/${name}`, import.meta.url)) };
}

/** The business units of each group company, with their logos. */
const GROUP_UNITS: Record<string, { code: string; name: string; colour: string; logo_document_id: string | null }[]> = {
  FZIP01: [{ code: 'FZ', name: 'Factory Zero', colour: '#3D5A80', logo_document_id: 'DOCFZ' }],
  PTOP01: [
    { code: 'LDG', name: 'Ledgers', colour: '#7FA6C9', logo_document_id: 'DOCLDG' },
    { code: 'SQZ', name: 'Sqeezin', colour: '#E9B44C', logo_document_id: null },
  ],
};

type Reply = { status: number; body: unknown; problem?: boolean };

interface Member {
  company_id: string;
  kind: 'holding' | 'ip_holding' | 'operating';
  ownership_bp: number;
}

interface GroupRecord {
  id: string;
  name: string;
  members: Member[];
  logo_document_id?: string | null;
  logo_company_id?: string | null;
}

interface Docs {
  status: 'draft' | 'posted' | 'settled' | 'failed';
  gross: number;
  wht: number;
  last_error: string | null;
}

interface AgreementRecord {
  id: string;
  group_id: string;
  licensor_company_id: string;
  licensee_company_id: string;
  kind: 'licence' | 'royalty' | 'management_fee' | 'loan';
  basis: 'pct_of_revenue_bp' | 'fixed_monthly' | 'cost_plus_bp';
  rate_bp: number | null;
  fixed_minor: number | null;
  currency: string;
  treaty_override_bp: number | null;
  notes: string | null;
  accounts: { licensor: AccountsView; licensee: AccountsView };
  terminated_at: string | null;
  periods: Map<string, { licensor: Docs; licensee: Docs }>;
}

interface AccountsView {
  royalty: string;
  ic: string;
  wht: string | null;
  bank: string;
  unit: string | null;
}

/** The companies of the fake group world, with the country each one's settings name. */
export const GROUP_COMPANIES: Record<string, { name: string; country: string; role: string }> = {
  HALDEN01: { name: 'Halden Works GmbH', country: 'DE', role: 'owner' },
  FZIP01: { name: 'Factory Zero IP Pte. Ltd.', country: 'SG', role: 'owner' },
  PTOP01: { name: 'PT Operasional Nusantara', country: 'ID', role: 'owner' },
};

/** A company the person does not hold a role in: its name is never shown. */
const HIDDEN = { id: 'KESTREL9', name: 'Kestrel Holdings Ltd', country: 'GB' };

/** The fake's monthly revenue of the licensee, before the rate: EUR 20,000.00. */
const LICENSEE_REVENUE_MINOR = 2_000_000;

export const groupsMock: {
  enabled: boolean;
  groups: GroupRecord[];
  agreements: AgreementRecord[];
  /** The next post of this period fails on the licensee side (the test sets it). */
  failNextPost: string | null;
  /** Business units with logos for the group companies (off by default: the agreement form asks for a unit when a company has any). */
  unitsEnabled: boolean;
  /** Added to the licensee's payable in the reconciliation, to show a difference. */
  reconDifference: number;
  writes: string[];
  counter: number;
} = {
  enabled: false,
  unitsEnabled: false,
  groups: [],
  agreements: [],
  failNextPost: null,
  reconDifference: 0,
  writes: [],
  counter: 0,
};

const CHART: { number: string; name: string; type: string; postable: boolean; tags: string[] }[] = [
  { number: '1000', name: 'Bank', type: 'asset', postable: true, tags: ['bank'] },
  { number: '1200', name: 'Intercompany receivable', type: 'asset', postable: true, tags: [] },
  { number: '1210', name: 'Receivables (header)', type: 'asset', postable: false, tags: [] },
  { number: '1250', name: 'Withholding receivable', type: 'asset', postable: true, tags: [] },
  { number: '2070', name: 'Intercompany payable', type: 'liability', postable: true, tags: [] },
  { number: '2060', name: 'Withholding payable', type: 'liability', postable: true, tags: [] },
  { number: '4060', name: 'Royalty income', type: 'income', postable: true, tags: [] },
  { number: '6100', name: 'Royalty expense', type: 'expense', postable: true, tags: [] },
];

export function resetGroups(enabled: boolean): void {
  groupsMock.enabled = enabled;
  groupsMock.failNextPost = null;
  groupsMock.unitsEnabled = false;
  groupsMock.reconDifference = 0;
  groupsMock.writes = [];
  groupsMock.counter = 0;
  groupsMock.groups = [];
  groupsMock.agreements = [];
  if (!enabled) return;
  companyState.companies = Object.entries(GROUP_COMPANIES).map(
    ([id, company]): MockCompany => ({ id, name: company.name, baseCurrency: 'EUR', role: company.role }),
  );
  groupsMock.groups = [
    {
      id: 'GRP1',
      name: 'Factory Zero Group',
      members: [
        { company_id: 'FZIP01', kind: 'ip_holding', ownership_bp: 10000 },
        { company_id: 'PTOP01', kind: 'operating', ownership_bp: 10000 },
        { company_id: 'HALDEN01', kind: 'operating', ownership_bp: 6000 },
      ],
    },
  ];
  // The first agreement: the IP company licenses 5% of the operating company's revenue, Singapore to Indonesia.
  seedAgreement({
    id: 'AG-ID-1',
    licensor: 'FZIP01',
    licensee: 'PTOP01',
    periods: {
      '2026-01': { status: 'settled' },
      '2026-02': { status: 'posted' },
    },
  });
}

function seedAgreement(input: {
  id: string;
  licensor: string;
  licensee: string;
  periods: Record<string, { status: Docs['status'] }>;
}): void {
  const record: AgreementRecord = {
    id: input.id,
    group_id: 'GRP1',
    licensor_company_id: input.licensor,
    licensee_company_id: input.licensee,
    kind: 'royalty',
    basis: 'pct_of_revenue_bp',
    rate_bp: 500,
    fixed_minor: null,
    currency: 'EUR',
    treaty_override_bp: null,
    notes: null,
    accounts: {
      licensor: { royalty: '4060', ic: '1200', wht: '1250', bank: '1000', unit: null },
      licensee: { royalty: '6100', ic: '2070', wht: '2060', bank: '1000', unit: null },
    },
    terminated_at: null,
    periods: new Map(),
  };
  for (const [period, { status }] of Object.entries(input.periods)) {
    setDraft(record, period);
    const docs = record.periods.get(period);
    if (docs) {
      docs.licensor.status = status;
      docs.licensee.status = status;
    }
  }
  groupsMock.agreements.push(record);
}

/** The withholding the pack gives the payer's country, or null when the pack gives none. */
function packWhtBp(country: string): number | null {
  if (country === 'SG') return 1000;
  if (country === 'ID') return 2000;
  return null;
}

function problem(status: number, slug: string, title: string, detail: string): Reply {
  return { status, problem: true, body: { type: `https://ledgers.sh/problems/${slug}`, title, status, detail } };
}

function reply(status: number, body: unknown): Reply {
  return { status, body };
}

function companyRole(id: string): string | null {
  return companyState.companies.find((company) => company.id === id)?.role ?? null;
}

function groupView(group: GroupRecord) {
  const visible = group.members.filter((member) => companyRole(member.company_id) !== null);
  return {
    id: group.id,
    name: group.name,
    logo_document_id: group.logo_document_id ?? null,
    logo_company_id: group.logo_company_id ?? null,
    members: visible.map((member) => ({
      company_id: member.company_id,
      company_name: GROUP_COMPANIES[member.company_id]?.name ?? member.company_id,
      kind: member.kind,
      ownership_bp: member.ownership_bp,
    })),
    has_hidden_members: visible.length < group.members.length,
    created_by: 'person_01',
    created_at: '2026-10-01T09:00:00Z',
    updated_at: '2026-10-22T09:00:00Z',
  };
}

function sideOf(agreement: AgreementRecord, company: string): 'licensor' | 'licensee' | null {
  if (agreement.licensor_company_id === company) return 'licensor';
  if (agreement.licensee_company_id === company) return 'licensee';
  return null;
}

function agreementView(agreement: AgreementRecord, company: string) {
  const side = sideOf(agreement, company) ?? 'licensor';
  const counterparty = side === 'licensor' ? agreement.licensee_company_id : agreement.licensor_company_id;
  const licenseeCountry = GROUP_COMPANIES[agreement.licensee_company_id]?.country ?? HIDDEN.country;
  const licensorCountry = GROUP_COMPANIES[agreement.licensor_company_id]?.country ?? HIDDEN.country;
  const pack = packWhtBp(licenseeCountry);
  let whtBp = 0;
  let whtBasis: 'pack' | 'treaty_override' | 'resident' | 'no_rate_on_file' | 'not_applicable' = 'not_applicable';
  if (agreement.kind === 'licence' || agreement.kind === 'royalty') {
    if (licenseeCountry === licensorCountry) {
      whtBasis = 'resident';
    } else if (agreement.treaty_override_bp !== null) {
      whtBp = agreement.treaty_override_bp;
      whtBasis = 'treaty_override';
    } else if (pack !== null) {
      whtBp = pack;
      whtBasis = 'pack';
    } else {
      whtBasis = 'no_rate_on_file';
    }
  }
  const counterpartyVisible = companyRole(counterparty) !== null;
  return {
    id: agreement.id,
    group_id: agreement.group_id,
    side,
    counterparty_id: counterpartyVisible ? counterparty : null,
    counterparty_hidden: !counterpartyVisible,
    kind: agreement.kind,
    basis: agreement.basis,
    rate_bp: agreement.rate_bp,
    fixed_minor: agreement.fixed_minor,
    currency: agreement.currency,
    wht_bp: whtBp,
    wht_basis: whtBasis,
    treaty_override_bp: agreement.treaty_override_bp,
    accounts: agreement.accounts[side],
    notes: agreement.notes,
    terminated_at: agreement.terminated_at,
    created_by: 'person_01',
    created_at: '2026-10-01T09:00:00Z',
    updated_at: '2026-10-22T09:00:00Z',
  };
}

function docView(agreement: AgreementRecord, period: string, side: 'licensor' | 'licensee') {
  const docs = agreement.periods.get(period);
  const doc = docs?.[side];
  const gross = doc?.gross ?? 0;
  const wht = side === 'licensee' ? (doc?.wht ?? 0) : 0;
  return {
    id: `IC-${agreement.id}-${period}`,
    side,
    period,
    status: doc?.status ?? 'draft',
    currency: agreement.currency,
    basis_minor: LICENSEE_REVENUE_MINOR,
    gross_minor: gross,
    wht_minor: wht,
    net_minor: gross - wht,
    accrual_entry_id: doc && doc.status !== 'draft' ? `ic-${agreement.id}-${period}-${side}` : null,
    settle_entry_id: doc?.status === 'settled' ? `ic-${agreement.id}-${period}-${side}-settle` : null,
    last_error: doc?.last_error ?? null,
  };
}

function findAgreement(id: string): AgreementRecord | undefined {
  return groupsMock.agreements.find((agreement) => agreement.id === id);
}

/** Drafts one period: the gross from the rate (or the fixed amount), the withholding from the payer's country. */
function setDraft(agreement: AgreementRecord, period: string): void {
  if (agreement.periods.has(period)) return;
  const gross = agreement.fixed_minor ?? Math.round((LICENSEE_REVENUE_MINOR * (agreement.rate_bp ?? 0)) / 10_000);
  const payer = GROUP_COMPANIES[agreement.licensee_company_id]?.country ?? HIDDEN.country;
  const payee = GROUP_COMPANIES[agreement.licensor_company_id]?.country ?? HIDDEN.country;
  const royalty = agreement.kind === 'licence' || agreement.kind === 'royalty';
  const whtBp = !royalty || payer === payee ? 0 : (agreement.treaty_override_bp ?? packWhtBp(payer) ?? 0);
  const wht = Math.round((gross * whtBp) / 10_000);
  agreement.periods.set(period, {
    licensor: { status: 'draft', gross, wht: 0, last_error: null },
    licensee: { status: 'draft', gross, wht, last_error: null },
  });
}

/** The group routes, or null for any other path. `company` is the page's company (the header), or the person's. */
export function handleGroups(method: string, url: URL, bodyText: string, company: string, agentKey: boolean): Reply | null {
  const path = url.pathname;
  const body = bodyText ? (JSON.parse(bodyText) as Record<string, unknown>) : {};
  // Company settings and the chart of each company: read per company, for the withholding and the pickers.
  if (path === '/v1/company' && method === 'GET') {
    const info = GROUP_COMPANIES[company] ?? HIDDEN;
    return reply(200, {
      name: info.name,
      base_currency: 'EUR',
      base_currency_locked: true,
      profile: { legal_form: '', vat_id: '', registration_number: '', address_line: '', postal_code: '', city: '', country: info.country, contact_email: '' },
    });
  }
  if (path === '/v1/accounts' && method === 'GET') {
    return reply(200, {
      accounts: CHART.map((account) => ({
        number: account.number,
        name_key: `account.custom.${account.number}`,
        display_name: account.name,
        account_type: account.type,
        normal_balance: account.type === 'asset' || account.type === 'expense' ? 'debit' : 'credit',
        currency: null,
        parent: null,
        tags: account.tags,
        is_postable: account.postable,
        archived: false,
        balances: [],
      })),
    });
  }
  if (path === '/v1/units' && method === 'GET') {
    const units = groupsMock.unitsEnabled ? (GROUP_UNITS[company] ?? []).map((unit) => ({ ...unit, archived: false })) : [];
    return reply(200, { units });
  }
  if (path === '/v1/reports/units-summary' && method === 'GET') {
    const row = (unit: string | null, minor: number) => ({
      unit,
      revenue: { minor: String(minor), currency: 'EUR' },
      expenses: { minor: String(Math.round(minor / 2)), currency: 'EUR' },
      result: { minor: String(minor - Math.round(minor / 2)), currency: 'EUR' },
      cash: { minor: '0', currency: 'EUR' },
      external_revenue: { minor: String(minor), currency: 'EUR' },
    });
    const units = groupsMock.unitsEnabled ? (GROUP_UNITS[company] ?? []).map((unit, index) => row(unit.code, 1_200_000 - index * 300_000)) : [];
    return reply(200, { period: url.searchParams.get('period') ?? '', units, consolidated: row(null, 1_500_000), eliminations: row(null, 0) });
  }
  // A logo's signed link: the same shape the server returns for a stored document.
  const logoLink = /^\/v1\/documents\/([^/]+)\/url$/.exec(path);
  if (logoLink && method === 'GET' && LOGO_FILES[decodeURIComponent(logoLink[1] ?? '')]) {
    const expires = Math.floor(Date.now() / 1000) + 300;
    return reply(200, { url: `/v1/documents/${logoLink[1]}/file?company=${company}&expires=${expires}&signature=mock`, expires_at: expires });
  }

  if (agentKey) {
    return problem(403, 'human-only', 'Human-only', 'this command is human-only');
  }

  if (path === '/v1/groups' && method === 'GET') {
    // The groups this company belongs to, among those whose members the person may see.
    const groups = groupsMock.groups.filter(
      (group) => group.members.some((m) => m.company_id === company) && group.members.some((m) => companyRole(m.company_id) !== null),
    );
    return reply(200, { groups: groups.map(groupView) });
  }
  if (path === '/v1/groups' && method === 'POST') {
    const members = (body.members as Member[] | undefined) ?? [];
    if (members.length < 2) return problem(422, 'invalid-input', 'Invalid input', 'A group needs at least two companies.');
    for (const member of members) {
      if (companyRole(member.company_id) !== 'owner') {
        return problem(403, 'forbidden', 'Forbidden', `You are not an owner of ${GROUP_COMPANIES[member.company_id]?.name ?? member.company_id}. A group needs an owner of every member company.`);
      }
    }
    groupsMock.counter += 1;
    const group: GroupRecord = { id: `GRP${groupsMock.counter + 1}`, name: String(body.name), members };
    groupsMock.groups.push(group);
    groupsMock.writes.push(`group.create ${group.id}`);
    return reply(200, groupView(group));
  }
  const groupUpdate = /^\/v1\/groups\/([^/]+)\/update$/.exec(path);
  if (groupUpdate && method === 'POST') {
    const group = groupsMock.groups.find((item) => item.id === groupUpdate[1]);
    if (!group) return problem(404, 'not-found', 'Not found', 'No such group.');
    if (typeof body.name === 'string') group.name = body.name;
    if (Array.isArray(body.members)) group.members = body.members as Member[];
    groupsMock.writes.push(`group.update ${group.id}`);
    return reply(200, groupView(group));
  }
  const groupView2 = /^\/v1\/groups\/([^/]+)\/view$/.exec(path);
  if (groupView2 && method === 'GET') {
    const group = groupsMock.groups.find((item) => item.id === groupView2[1]);
    if (!group) return problem(404, 'not-found', 'Not found', 'No such group.');
    const period = url.searchParams.get('period') ?? '2026-02';
    const asOf = url.searchParams.get('as_of') ?? `${period}-28`;
    const members = group.members
      .filter((member) => companyRole(member.company_id) !== null)
      .map((member, index) => ({
        company: { id: member.company_id, name: GROUP_COMPANIES[member.company_id]?.name ?? member.company_id },
        kind: member.kind,
        ownership_bp: member.ownership_bp,
        currency: 'EUR',
        revenue_minor: 2_500_000 + index * 100_000,
        expenses_minor: 1_800_000 + index * 50_000,
        net_income_minor: 700_000 + index * 50_000,
        assets_minor: 9_000_000 + index * 200_000,
        liabilities_minor: 4_000_000,
        equity_minor: 5_000_000 + index * 200_000,
        balanced: true,
      }));
    return reply(200, { group_id: group.id, name: group.name, period, as_of: asOf, members, consolidated: 'Combined consolidation is a Ledgers Cloud feature.' });
  }
  const reconciliation = /^\/v1\/groups\/([^/]+)\/reconciliation$/.exec(path);
  if (reconciliation && method === 'GET') {
    const group = groupsMock.groups.find((item) => item.id === reconciliation[1]);
    if (!group) return problem(404, 'not-found', 'Not found', 'No such group.');
    const pairs = groupsMock.agreements
      .filter((agreement) => agreement.group_id === group.id && companyRole(agreement.licensor_company_id) !== null && companyRole(agreement.licensee_company_id) !== null)
      .map((agreement) => {
        const periods = [...agreement.periods.entries()].sort(([a], [b]) => (a < b ? -1 : 1));
        const open = periods.filter(([, docs]) => docs.licensee.status !== 'settled').map(([period]) => period);
        // The books: receivable and payable (gross, while not settled); income and expense (gross, for good).
        let receivable = 0;
        let payable = 0;
        let income = 0;
        let expense = 0;
        for (const [, docs] of periods) {
          if (docs.licensor.status === 'posted') receivable += docs.licensor.gross;
          if (docs.licensee.status === 'posted') payable += docs.licensee.gross;
          if (docs.licensor.status === 'posted' || docs.licensor.status === 'settled') income += docs.licensor.gross;
          if (docs.licensee.status === 'posted' || docs.licensee.status === 'settled') expense += docs.licensee.gross;
        }
        payable += groupsMock.reconDifference;
        return {
          agreement_id: agreement.id,
          group_id: group.id,
          licensor: { id: agreement.licensor_company_id, name: GROUP_COMPANIES[agreement.licensor_company_id]?.name ?? '' },
          licensee: { id: agreement.licensee_company_id, name: GROUP_COMPANIES[agreement.licensee_company_id]?.name ?? '' },
          currency: 'EUR',
          receivable_minor: receivable,
          payable_minor: payable,
          balance_difference_minor: receivable - payable,
          income_minor: income,
          expense_minor: expense,
          income_difference_minor: income - expense,
          status: receivable === payable && income === expense ? 'balanced' : 'difference',
          open_periods: open,
          periods: periods.map(([period, docs]) => ({
            period,
            licensor_status: docs.licensor.status,
            licensee_status: docs.licensee.status,
            gross_minor: docs.licensor.gross,
            wht_minor: docs.licensee.wht,
            net_minor: docs.licensor.gross - docs.licensee.wht,
          })),
        };
      });
    return reply(200, { group_id: group.id, pairs });
  }

  if (path === '/v1/agreements' && method === 'GET') {
    return reply(200, {
      agreements: groupsMock.agreements.filter((agreement) => sideOf(agreement, company) !== null).map((agreement) => agreementView(agreement, company)),
    });
  }
  if (path === '/v1/agreements' && method === 'POST') {
    const licensor = String(body.licensor_company_id);
    const licensee = String(body.licensee_company_id);
    for (const id of [licensor, licensee]) {
      if (!['owner', 'admin'].includes(companyRole(id) ?? '')) {
        return problem(403, 'forbidden', 'Forbidden', 'Changing an agreement needs an owner or an admin in both companies.');
      }
    }
    groupsMock.counter += 1;
    const id = `AG-NEW-${groupsMock.counter}`;
    const record: AgreementRecord = {
      id,
      group_id: String(body.group_id),
      licensor_company_id: licensor,
      licensee_company_id: licensee,
      kind: body.kind as AgreementRecord['kind'],
      basis: body.basis as AgreementRecord['basis'],
      rate_bp: (body.rate_bp as number | null) ?? null,
      fixed_minor: (body.fixed_minor as number | null) ?? null,
      currency: String(body.currency),
      treaty_override_bp: (body.treaty_override_bp as number | null) ?? null,
      notes: (body.notes as string | null) ?? null,
      accounts: {
        licensor: body.licensor as AccountsView,
        licensee: body.licensee as AccountsView,
      },
      terminated_at: null,
      periods: new Map(),
    };
    groupsMock.agreements.push(record);
    groupsMock.writes.push(`agreement.create ${id}`);
    return reply(200, { licensor: agreementView(record, licensor), licensee: agreementView(record, licensee) });
  }
  const agreementPath = /^\/v1\/agreements\/([^/]+)(?:\/([a-z-]+))?$/.exec(path);
  if (agreementPath) {
    const agreement = findAgreement(decodeURIComponent(agreementPath[1] ?? ''));
    const action = agreementPath[2];
    if (!agreement) return problem(404, 'not-found', 'Not found', 'No such agreement.');
    if (sideOf(agreement, company) === null) return problem(404, 'not-found', 'Not found', 'No such agreement.');
    if (method === 'GET' && !action) {
      const periods = [...agreement.periods.keys()].sort().flatMap((period) => [docView(agreement, period, 'licensor'), docView(agreement, period, 'licensee')]);
      return reply(200, { agreement: agreementView(agreement, company), periods });
    }
    if (method === 'POST' && action === 'update') {
      agreement.kind = body.kind as AgreementRecord['kind'];
      agreement.basis = body.basis as AgreementRecord['basis'];
      agreement.rate_bp = (body.rate_bp as number | null) ?? null;
      agreement.fixed_minor = (body.fixed_minor as number | null) ?? null;
      agreement.treaty_override_bp = (body.treaty_override_bp as number | null) ?? null;
      agreement.notes = (body.notes as string | null) ?? null;
      agreement.accounts = { licensor: body.licensor as AccountsView, licensee: body.licensee as AccountsView };
      groupsMock.writes.push(`agreement.update ${agreement.id}`);
      return reply(200, agreementView(agreement, company));
    }
    if (method === 'POST' && action === 'terminate') {
      agreement.terminated_at = '2026-10-22T10:00:00Z';
      return reply(200, agreementView(agreement, company));
    }
    if (method === 'POST' && (action === 'run-period' || action === 'post-period' || action === 'settle-period')) {
      const periods = (body.periods as string[] | undefined) ?? [];
      const results: { period: string; docs: ReturnType<typeof docView>[] }[] = [];
      if (action === 'run-period') {
        if (agreement.terminated_at) return problem(422, 'invalid-input', 'Invalid input', 'The agreement has ended.');
        for (const period of periods) setDraft(agreement, period);
        groupsMock.writes.push(`agreement.run ${agreement.id} ${periods.join(',')}`);
        for (const period of periods) results.push({ period, docs: [docView(agreement, period, 'licensor'), docView(agreement, period, 'licensee')] });
        return reply(200, { agreement_id: agreement.id, periods: results });
      }
      if (action === 'post-period') {
        for (const period of periods) {
          const docs = agreement.periods.get(period);
          if (!docs) return problem(422, 'invalid-input', 'Invalid input', `${period} has not been drafted.`);
          if (docs.licensor.status === 'posted' || docs.licensor.status === 'settled') continue;
          if (groupsMock.failNextPost === period) {
            // The licensee refuses: its post fails, the licensor's entry is reversed, and both drafts go to failed.
            groupsMock.failNextPost = null;
            const reason = `The licensee's account 2070 is locked for ${period} (test reason).`;
            docs.licensor = { ...docs.licensor, status: 'failed', last_error: `Reversed: ${reason}` };
            docs.licensee = { ...docs.licensee, status: 'failed', last_error: reason };
            groupsMock.writes.push(`agreement.post-failed ${agreement.id} ${period}`);
            return problem(422, 'invalid-input', 'Invalid input', reason);
          }
          docs.licensor = { ...docs.licensor, status: 'posted', last_error: null };
          docs.licensee = { ...docs.licensee, status: 'posted', last_error: null };
          groupsMock.writes.push(`agreement.post ${agreement.id} ${period}`);
        }
        for (const period of periods) results.push({ period, docs: [docView(agreement, period, 'licensor'), docView(agreement, period, 'licensee')] });
        return reply(200, { agreement_id: agreement.id, periods: results });
      }
      for (const period of periods) {
        const docs = agreement.periods.get(period);
        if (!docs || docs.licensee.status !== 'posted') {
          return problem(422, 'invalid-input', 'Invalid input', `${period} is not posted in both companies.`);
        }
        docs.licensor = { ...docs.licensor, status: 'settled' };
        docs.licensee = { ...docs.licensee, status: 'settled' };
        groupsMock.writes.push(`agreement.settle ${agreement.id} ${period}`);
      }
      for (const period of periods) results.push({ period, docs: [docView(agreement, period, 'licensor'), docView(agreement, period, 'licensee')] });
      return reply(200, { agreement_id: agreement.id, periods: results });
    }
  }
  return null;
}
