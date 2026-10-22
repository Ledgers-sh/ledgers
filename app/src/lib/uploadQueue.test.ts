import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { flushUploads, memoryStore, type QueuedUpload } from './uploadQueue';

const config = (fetchImpl: typeof fetch) => ({ baseUrl: 'http://api.test', fetch: fetchImpl, apiKey: 'k' });

function entry(sha256: string, name = `${sha256}.pdf`): QueuedUpload {
  return { sha256, name, type: 'application/pdf', blob: new Blob(['%PDF-1.4']), queuedAt: '2026-10-22T10:00:00Z' };
}

/** A fake server: stores one document per hash, answers duplicates, and drafts a bill per document. */
function fakeServer(options: { failUploadsOnce?: boolean } = {}) {
  const docs = new Map<string, string>();
  const bills = new Map<string, string>();
  let uploads = 0;
  let failed = false;
  const fetchImpl = vi.fn(async (input: RequestInfo | URL, request?: RequestInit) => {
    const init = request ?? {};
    const path = new URL(String(input)).pathname;
    if (path === '/v1/documents/upload') {
      if (options.failUploadsOnce && !failed) {
        failed = true;
        throw new TypeError('network down');
      }
      uploads += 1;
      const sha = String((init.headers as Record<string, string>)['X-File-Name']);
      const hash = sha.replace(/\.pdf$/, '');
      const existing = docs.get(hash);
      const id = existing ?? `doc-${hash}`;
      docs.set(hash, id);
      return new Response(JSON.stringify({ document: { id }, duplicate: existing !== undefined, warning: null }), {
        status: existing ? 200 : 201,
        headers: { 'Content-Type': 'application/json' },
      });
    }
    if (path === '/v1/bills/extract') {
      const body = JSON.parse(String(init.body)) as { document_id: string };
      const existing = bills.get(body.document_id);
      if (existing) {
        return new Response(JSON.stringify({ type: 'x', detail: 'duplicate', existing_bill_id: existing }), {
          status: 409,
          headers: { 'Content-Type': 'application/problem+json' },
        });
      }
      bills.set(body.document_id, `bill-${body.document_id}`);
      return new Response(JSON.stringify({ bill: { id: `bill-${body.document_id}` }, document: {}, extraction: { mode: 'manual', warnings: [] } }), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      });
    }
    return new Response('{}', { status: 404 });
  });
  return { fetchImpl, uploads: () => uploads, bills };
}

describe('upload queue', () => {
  beforeEach(() => {
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
  });
  afterEach(() => vi.restoreAllMocks());

  it('uploads each waiting file once and drafts one bill from it', async () => {
    const store = memoryStore();
    await store.put(entry('aaa'));
    await store.put(entry('bbb'));
    const server = fakeServer();
    const report = await flushUploads(config(server.fetchImpl), store);
    expect(report.uploaded).toBe(2);
    expect(report.extracted).toBe(2);
    expect(report.pending).toBe(0);
    expect(await store.list()).toEqual([]);
    expect(server.uploads()).toBe(2);
    // A second flush with nothing waiting uploads nothing.
    await flushUploads(config(server.fetchImpl), store);
    expect(server.uploads()).toBe(2);
  });

  it('keeps everything queued when the network drops, then uploads each file exactly once on retry', async () => {
    const store = memoryStore();
    await store.put(entry('aaa'));
    await store.put(entry('bbb'));
    const server = fakeServer({ failUploadsOnce: true });
    const first = await flushUploads(config(server.fetchImpl), store);
    expect(first.pending).toBe(2);
    expect(server.uploads()).toBe(0);
    const second = await flushUploads(config(server.fetchImpl), store);
    expect(second.pending).toBe(0);
    expect(server.uploads()).toBe(2);
    expect(await store.list()).toEqual([]);
  });

  it('treats a file that was already stored as done and links the bill it already has', async () => {
    const store = memoryStore();
    const server = fakeServer();
    await store.put(entry('aaa'));
    await flushUploads(config(server.fetchImpl), store);
    // The same file again, as after a dropped response: one document, no second bill.
    await store.put(entry('aaa'));
    const report = await flushUploads(config(server.fetchImpl), store);
    expect(report.duplicates).toBe(1);
    expect(report.billIds).toEqual(['bill-doc-aaa']);
    expect(await store.list()).toEqual([]);
  });
});
