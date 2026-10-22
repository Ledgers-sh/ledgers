/**
 * The bills and documents routes of the fake API (issues #20 to #22, #32, #34).
 * The extraction is canned: the fixture invoice reads as the rules would read
 * it (header fields, no lines, so it needs review); the fixture photo reads as
 * nothing (no model key), so it is entered by hand. No model is called.
 */
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const FIXTURES = join(here, '..', 'fixtures', 'bills');

export interface MockReply {
  status: number;
  body: unknown;
  problem?: boolean;
}

/** A file the mock serves on GET /v1/documents/{id}/file. */
export interface ServedFile {
  mime: string;
  bytes: Buffer;
}

interface MockDocument {
  id: string;
  sha256: string;
  size_bytes: number;
  mime: string;
  original_name: string;
  uploaded_by: string;
  retention_class: 'ten-years';
  created_at: string;
  has_text_layer: boolean;
  file: Buffer;
}

interface MockBill {
  id: string;
  document_id: string;
  vendor: { id: string; name: string; vat_id: string | null; iban: string | null; needs_review: boolean };
  invoice_number: string | null;
  invoice_date: string | null;
  due_date: string | null;
  currency: string;
  net_minor: number | null;
  vat_minor: number | null;
  gross_minor: number | null;
  lines: Array<{ description: string; net_minor: number | null; vat_rate_bp: number | null; vat_minor: number | null; gross_minor: number | null; expense_account: string | null; suggested_from: string | null }>;
  confidence: Record<string, number>;
  source: string;
  posted_entry_id: string | null;
  discarded: boolean;
  created_at: string;
  updated_at: string;
  entry?: { entry_id: string; number: number };
}

export interface BillsMock {
  documents: MockDocument[];
  bills: MockBill[];
  /** How many times the upload route answered (exactly-once checks). */
  uploads: number;
  /** How many times the model extractor was asked (always zero: no model key in tests). */
  modelCalls: number;
  /** Bill ids that were posted, in order. */
  posts: string[];
}

export const billsMock: BillsMock = { documents: [], bills: [], uploads: 0, modelCalls: 0, posts: [] };

export function resetBills(): void {
  billsMock.documents = [];
  billsMock.bills = [];
  billsMock.uploads = 0;
  billsMock.modelCalls = 0;
  billsMock.posts = [];
}

/** The fixture files, by name. Read once per process. */
const FILES: Record<string, { mime: string; bytes: Buffer }> = {
  'invoice.pdf': { mime: 'application/pdf', bytes: readFileSync(join(FIXTURES, 'invoice.pdf')) },
  'scan.png': { mime: 'image/png', bytes: readFileSync(join(FIXTURES, 'scan.png')) },
};

function sha(bytes: Buffer): string {
  return createHash('sha256').update(bytes).digest('hex');
}

function problem(status: number, title: string, detail?: string, extra: Record<string, unknown> = {}): MockReply {
  return { status, problem: true, body: { type: 'about:blank', title, status, detail, ...extra } };
}

function documentView(doc: MockDocument) {
  const { file: _file, ...view } = doc;
  return view;
}

function billView(bill: MockBill) {
  const flags: string[] = [];
  if (bill.vendor.needs_review) flags.push('vendor-new');
  if (bill.lines.some((line) => line.expense_account === null) && bill.lines.length > 0) flags.push('no-expense-account');
  const checks = [
    { id: 'currency', ok: bill.currency === 'EUR', detail: `${bill.currency} is a known currency` },
    { id: 'totals-present', ok: bill.net_minor !== null && bill.vat_minor !== null && bill.gross_minor !== null, detail: 'net, VAT and gross are present' },
    {
      id: 'gross-equals-net-plus-vat',
      ok: bill.net_minor !== null && bill.vat_minor !== null && bill.gross_minor !== null && bill.net_minor + bill.vat_minor === bill.gross_minor,
      detail: 'net plus VAT equals gross',
    },
    { id: 'lines-present', ok: bill.lines.length > 0, detail: bill.lines.length > 0 ? `${bill.lines.length} lines` : 'no invoice lines were extracted' },
    {
      id: 'amounts-in-document',
      ok: true,
      detail: 'every total appears in the document text',
    },
  ];
  const checksOk = checks.every((check) => check.ok);
  const status = bill.posted_entry_id !== null ? 'posted' : bill.discarded ? 'discarded' : checksOk && flags.length === 0 ? 'ready' : 'needs-review';
  const journal = bill.lines.every((line) => line.expense_account !== null) && bill.lines.length > 0 && bill.vat_minor !== null && bill.gross_minor !== null
    ? [
        ...bill.lines.map((line) => ({ account: line.expense_account as string, side: 'debit', minor: line.net_minor as number })),
        { account: '1050', side: 'debit', minor: bill.vat_minor },
        { account: '2000', side: 'credit', minor: bill.gross_minor },
      ]
    : [];
  return {
    id: bill.id,
    status,
    document_id: bill.document_id,
    vendor: bill.vendor,
    invoice_number: bill.invoice_number,
    invoice_date: bill.invoice_date,
    due_date: bill.due_date,
    currency: bill.currency,
    net_minor: bill.net_minor,
    vat_minor: bill.vat_minor,
    gross_minor: bill.gross_minor,
    lines: bill.lines,
    checks,
    flags,
    confidence: bill.confidence,
    source: bill.source,
    posted_entry_id: bill.posted_entry_id,
    journal_preview: bill.posted_entry_id === null ? journal : [],
    created_by: 'person:person-owner',
    created_at: bill.created_at,
    updated_at: bill.updated_at,
  };
}

function findBill(id: string): MockBill | undefined {
  return billsMock.bills.find((bill) => bill.id === id);
}

/** The canned reading of a fixture: the header fields and the totals the text layer carries. */
function readingOf(name: string): Pick<MockBill, 'vendor' | 'invoice_number' | 'invoice_date' | 'due_date' | 'currency' | 'net_minor' | 'vat_minor' | 'gross_minor' | 'lines' | 'confidence' | 'source'> {
  if (name === 'invoice.pdf') {
    return {
      vendor: { id: '', name: 'Halden Supplies GmbH', vat_id: 'DE123456789', iban: 'DE89370400440532013000', needs_review: true },
      invoice_number: 'RE-2026-0042',
      invoice_date: '2026-10-15',
      due_date: '2026-11-14',
      currency: 'EUR',
      net_minor: 100_000,
      vat_minor: 19_000,
      gross_minor: 119_000,
      lines: [],
      confidence: { vendor_name: 40, vendor_vat_id: 70, vendor_iban: 70, invoice_number: 70, invoice_date: 70, due_date: 70, net_minor: 70, vat_minor: 70, gross_minor: 70 },
      source: 'rules',
    };
  }
  // A photo: no text layer, no model key. The fields are typed in by hand.
  return {
    vendor: { id: '', name: 'Scanned vendor', vat_id: null, iban: null, needs_review: true },
    invoice_number: null,
    invoice_date: null,
    due_date: null,
    currency: 'EUR',
    net_minor: null,
    vat_minor: null,
    gross_minor: null,
    lines: [],
    confidence: {},
    source: 'manual',
  };
}

/** Handles the bills and documents routes. Returns null when the path is not one of them. */
export function handleBills(method: string, url: URL, bodyText: string, raw: Buffer, headers: Record<string, string | string[] | undefined>): MockReply | null {
  const path = url.pathname;
  const now = '2026-10-22T10:00:00Z';

  if (method === 'POST' && path === '/v1/documents/upload') {
    billsMock.uploads += 1;
    const name = decodeURIComponent(String(headers['x-file-name'] ?? 'document'));
    const bytes = raw;
    const digest = sha(bytes);
    const existing = billsMock.documents.find((doc) => doc.sha256 === digest);
    if (existing) {
      return {
        status: 200,
        body: {
          document: documentView(existing),
          duplicate: true,
          warning: 'This file was already stored. It is linked to the existing document and no second copy was kept.',
        },
      };
    }
    const mime = /%PDF-/.test(bytes.subarray(0, 5).toString('latin1')) ? 'application/pdf' : bytes[0] === 0x89 ? 'image/png' : bytes[0] === 0xff ? 'image/jpeg' : null;
    if (mime === null) return problem(400, 'Request validation failed', 'only PDF, PNG and JPEG files can be stored; this file is not one of them');
    const doc: MockDocument = {
      id: `doc_${billsMock.documents.length + 1}`,
      sha256: digest,
      size_bytes: bytes.length,
      mime,
      original_name: name,
      uploaded_by: 'person:person-owner',
      retention_class: 'ten-years',
      created_at: now,
      has_text_layer: mime === 'application/pdf',
      file: bytes,
    };
    billsMock.documents.push(doc);
    return { status: 201, body: { document: documentView(doc), duplicate: false, warning: null } };
  }

  const docFile = /^\/v1\/documents\/([^/]+)\/(url|file)$/.exec(path);
  if (method === 'GET' && docFile) {
    const doc = billsMock.documents.find((d) => d.id === decodeURIComponent(docFile[1]));
    if (!doc) return problem(404, 'Not found');
    if (docFile[2] === 'url') {
      return { status: 200, body: { url: `/v1/documents/${doc.id}/file?expires=${Date.parse(now) / 1000 + 300}&signature=mock`, expires_at: Date.parse(now) / 1000 + 300 } };
    }
    return { status: 200, body: { served: true } };
  }
  const docGet = /^\/v1\/documents\/([^/]+)$/.exec(path);
  if (method === 'GET' && docGet) {
    const doc = billsMock.documents.find((d) => d.id === decodeURIComponent(docGet[1]));
    return doc ? { status: 200, body: { document: documentView(doc) } } : problem(404, 'Not found');
  }

  if (method === 'POST' && path === '/v1/bills/extract') {
    const input = parse(bodyText);
    const doc = billsMock.documents.find((d) => d.id === input.document_id);
    if (!doc) return problem(404, 'Not found');
    const existing = billsMock.bills.find((bill) => bill.document_id === doc.id);
    if (existing) {
      return problem(409, 'This bill is already stored', `it matches bill ${existing.id} (the same document)`, { existing_bill_id: existing.id, reason: 'the same document' });
    }
    const reading = readingOf(doc.original_name);
    const mode = doc.mime === 'application/pdf' ? 'text-layer' : 'manual';
    const bill: MockBill = {
      id: `bill_${billsMock.bills.length + 1}`,
      document_id: doc.id,
      ...reading,
      vendor: { ...reading.vendor, id: `ven_${billsMock.bills.length + 1}` },
      posted_entry_id: null,
      discarded: false,
      created_at: now,
      updated_at: now,
    };
    billsMock.bills.push(bill);
    return {
      status: 200,
      body: {
        bill: billView(bill),
        document: documentView(doc),
        extraction: {
          mode,
          warnings:
            mode === 'manual'
              ? ['No model key is configured, so only the PDF text layer is read. Scans and images need the fields typed in by hand.']
              : ['No model key is configured, so only the PDF text layer is read. Scans and images need the fields typed in by hand.'],
        },
      },
    };
  }

  if (method === 'GET' && path === '/v1/bills') {
    return { status: 200, body: { bills: billsMock.bills.map(billView) } };
  }

  const billPath = /^\/v1\/bills\/([^/]+)(?:\/(update|confirm|post|discard))?$/.exec(path);
  if (billPath) {
    const bill = findBill(decodeURIComponent(billPath[1]));
    if (!bill) return problem(404, 'Not found');
    const action = billPath[2];
    if (method === 'GET' && !action) return { status: 200, body: { bill: billView(bill) } };
    if (method === 'POST' && action === 'update') {
      if (bill.posted_entry_id !== null) return problem(422, 'Invalid input', 'a posted bill is not edited; post a reversal and a new bill');
      const input = parse(bodyText);
      bill.invoice_number = input.invoice_number ?? null;
      bill.invoice_date = input.invoice_date ?? null;
      bill.due_date = input.due_date ?? null;
      bill.net_minor = input.net_minor ?? null;
      bill.vat_minor = input.vat_minor ?? null;
      bill.gross_minor = input.gross_minor ?? null;
      bill.lines = (input.lines ?? []).map((line: BillLineInput) => ({ ...line, suggested_from: line.suggested_from ?? null }));
      bill.vendor.name = input.vendor_name ?? bill.vendor.name;
      bill.updated_at = now;
      return { status: 200, body: { bill: billView(bill) } };
    }
    if (method === 'POST' && action === 'confirm') {
      bill.vendor.needs_review = false;
      bill.updated_at = now;
      return { status: 200, body: { bill: billView(bill) } };
    }
    if (method === 'POST' && action === 'discard') {
      if (bill.posted_entry_id !== null) return problem(422, 'Invalid input', 'a posted bill is not discarded; post a reversal instead');
      bill.discarded = true;
      bill.updated_at = now;
      return { status: 200, body: { bill: billView(bill) } };
    }
    if (method === 'POST' && action === 'post') {
      const view = billView(bill);
      if (view.status !== 'ready') {
        return problem(422, 'Invalid input', 'the bill needs review before it posts');
      }
      const number = 40 + billsMock.posts.length + 1;
      bill.posted_entry_id = `ent_${bill.id}`;
      bill.entry = { entry_id: bill.posted_entry_id, number };
      billsMock.posts.push(bill.id);
      bill.updated_at = now;
      return {
        status: 200,
        body: {
          bill: billView(bill),
          entry: { entry_id: bill.posted_entry_id, journal_id: 'general', fiscal_year: 2026, number, recorded_at: now },
        },
      };
    }
  }
  return null;
}

interface BillLineInput {
  description: string;
  net_minor: number | null;
  vat_rate_bp: number | null;
  vat_minor: number | null;
  gross_minor: number | null;
  expense_account: string | null;
  suggested_from?: string | null;
}

function parse(text: string): Record<string, any> {
  try {
    return text ? (JSON.parse(text) as Record<string, any>) : {};
  } catch {
    return {};
  }
}

/** The served bytes of a document, for GET /v1/documents/{id}/file. */
export function servedFile(path: string): ServedFile | null {
  const match = /^\/v1\/documents\/([^/]+)\/file/.exec(path);
  if (!match) return null;
  const doc = billsMock.documents.find((d) => d.id === decodeURIComponent(match[1]));
  return doc ? { mime: doc.mime, bytes: doc.file } : null;
}

/** The fixture file names, for specs that upload them. */
export const FIXTURE_NAMES = Object.keys(FILES);

/** The bytes of a fixture file. */
export function fixtureBytes(name: string): Buffer {
  const file = FILES[name];
  if (!file) throw new Error(`no fixture ${name}`);
  return file.bytes;
}
