/**
 * The business-unit routes of the fake API: units.list, the three unit writes
 * (a person only), entries.transfer and reports.units_summary. Units exist only
 * when a test asks for them (`units: true`), so the other screens keep their
 * single-company shape.
 */

export interface MockUnit {
  code: string;
  name: string;
  colour: string;
  logo_document_id: string | null;
  archived: boolean;
  created_at: string;
}

export interface UnitsReply {
  status: number;
  body: unknown;
  problem?: boolean;
}

/** Every transfer the fake API has accepted, for assertions. */
export const unitState: { units: MockUnit[]; transfers: unknown[]; summaryPeriods: string[] } = {
  units: [],
  transfers: [],
  summaryPeriods: [],
};

const SEED: MockUnit[] = [
  { code: 'FZ', name: 'Factory Zero holding', colour: '#E9B44C', logo_document_id: null, archived: false, created_at: '2026-01-05T09:00:00Z' },
  { code: 'LDG', name: 'Ledgers', colour: '#7FA6C9', logo_document_id: null, archived: false, created_at: '2026-01-05T09:01:00Z' },
  { code: 'SQZ', name: 'Sqeezin', colour: '#8FBF8A', logo_document_id: null, archived: false, created_at: '2026-01-05T09:02:00Z' },
];

export function resetUnits(seed: boolean): void {
  unitState.units = seed ? SEED.map((unit) => ({ ...unit })) : [];
  unitState.transfers = [];
  unitState.summaryPeriods = [];
}

function reply(status: number, body: unknown): UnitsReply {
  return { status, body };
}

function problem(status: number, slug: string, title: string, detail: string): UnitsReply {
  return { status, problem: true, body: { type: `https://ledgers.sh/problems/${slug}`, title, status, detail } };
}

const CODE = /^[A-Za-z0-9]{2,16}$/;
const COLOUR = /^#[0-9A-Fa-f]{6}$/;

/** The units summary for a month: the fixture figures, eliminations by the same rule as the server (consolidated minus units). */
function summary(period: string) {
  const row = (unit: string | null, revenue: number, expenses: number, cash: number) => ({
    unit,
    revenue: { minor: String(revenue * 100), currency: 'SGD' },
    expenses: { minor: String(expenses * 100), currency: 'SGD' },
    result: { minor: String((revenue - expenses) * 100), currency: 'SGD' },
    cash: { minor: String(cash * 100), currency: 'SGD' },
  });
  const units = [row('FZ', 5200, 3100, 22000), row('LDG', 3100, 1900, 9400), row('SQZ', 900, 1250, -1800)];
  const eliminations = row(null, -600, -600, 0);
  const consolidated = row(null, 5200 + 3100 + 900 - 600, 3100 + 1900 + 1250 - 600, 29600);
  return { period, units, consolidated, eliminations };
}

/** Answers a business-unit route, or returns null for a path it does not own. `agent` is a request with an agent key. */
export function handleUnits(method: string, url: URL, bodyText: string, agent: boolean): UnitsReply | null {
  const path = url.pathname;
  if (method === 'GET' && path === '/v1/units') return reply(200, { units: unitState.units });

  if (method === 'GET' && path === '/v1/reports/units-summary') {
    const period = url.searchParams.get('period') ?? '';
    if (!/^\d{4}-\d{2}$/.test(period)) return problem(422, 'validation-failed', 'Validation failed', 'period must be YYYY-MM');
    unitState.summaryPeriods.push(period);
    return reply(200, summary(period));
  }

  if (method === 'POST' && path === '/v1/transfers') {
    const body = JSON.parse(bodyText || '{}') as Record<string, string>;
    const known = (code: string) => unitState.units.some((unit) => unit.code === code && !unit.archived);
    if (!known(body.from_unit) || !known(body.to_unit)) return problem(404, 'unit-not-found', 'Business unit not found', 'No business unit of this company has that code.');
    if (body.from_unit === body.to_unit) return problem(422, 'unit-refused', 'Business unit operation refused', 'A recharge needs two different units.');
    unitState.transfers.push(body);
    return reply(200, {
      entry: {
        entry_id: `tr_${unitState.transfers.length}`,
        journal_id: 'general',
        fiscal_year: Number(String(body.posting_date).slice(0, 4)),
        number: unitState.transfers.length,
        recorded_at: '2026-10-23T10:00:00Z',
      },
    });
  }

  if (path === '/v1/units' && method === 'POST') {
    if (agent) return problem(403, 'human-only', 'Human-only', 'units.create changes business units; only a person may');
    const body = JSON.parse(bodyText || '{}') as { code?: string; name?: string; colour?: string; logo_document_id?: string };
    const code = (body.code ?? '').trim().toUpperCase();
    if (!CODE.test(code)) return problem(422, 'unit-refused', 'Business unit operation refused', 'invalid business unit: the code is 2 to 16 letters or digits');
    if (unitState.units.some((unit) => unit.code === code)) return problem(409, 'unit-conflict', 'Business unit conflict', `business unit ${code} already exists`);
    const unit: MockUnit = {
      code,
      name: (body.name ?? '').trim(),
      colour: body.colour && COLOUR.test(body.colour) ? body.colour.toUpperCase() : '#7FA6C9',
      logo_document_id: body.logo_document_id ?? null,
      archived: false,
      created_at: '2026-10-23T10:00:00Z',
    };
    unitState.units.push(unit);
    return reply(200, { unit });
  }

  const rename = /^\/v1\/units\/([^/]+)\/rename$/.exec(path);
  if (rename && method === 'POST') {
    if (agent) return problem(403, 'human-only', 'Human-only', 'units.rename changes business units; only a person may');
    const found = unitState.units.find((unit) => unit.code === decodeURIComponent(rename[1]));
    if (!found) return problem(404, 'unit-not-found', 'Business unit not found', 'No business unit of this company has that code.');
    if (found.archived) return problem(409, 'unit-conflict', 'Business unit conflict', `business unit ${found.code} is archived`);
    const body = JSON.parse(bodyText || '{}') as { name?: string; colour?: string; logo_document_id?: string; clear_logo?: boolean };
    if (body.name !== undefined) found.name = body.name.trim();
    if (body.colour !== undefined) found.colour = body.colour.toUpperCase();
    if (body.clear_logo) found.logo_document_id = null;
    if (body.logo_document_id !== undefined) found.logo_document_id = body.logo_document_id;
    return reply(200, { unit: found });
  }

  const archive = /^\/v1\/units\/([^/]+)\/archive$/.exec(path);
  if (archive && method === 'POST') {
    if (agent) return problem(403, 'human-only', 'Human-only', 'units.archive changes business units; only a person may');
    const found = unitState.units.find((unit) => unit.code === decodeURIComponent(archive[1]));
    if (!found) return problem(404, 'unit-not-found', 'Business unit not found', 'No business unit of this company has that code.');
    found.archived = true;
    return reply(200, { unit: found });
  }

  return null;
}
