/**
 * The upload queue (issue #34). Captured, shared and dropped files wait here
 * in IndexedDB until the server accepts them, then upload once: the queue key
 * is the file's SHA-256, an entry is removed only after the server answered,
 * and the server itself keeps one copy per hash, so a retry after a dropped
 * response records a second upload event and no second document.
 *
 * The service worker writes shared files into the same database (see
 * public/sw.js); the schema below is shared with it.
 */
import { ApiError, type ClientConfig } from '../api/client';
import { extractBill, uploadDocument, BillApiError } from '../api/bills';

export const QUEUE_DB = 'ledgers-uploads';
export const QUEUE_STORE = 'uploads';

/** One waiting file. `sha256` is the key. */
export interface QueuedUpload {
  sha256: string;
  name: string;
  type: string;
  blob: Blob;
  queuedAt: string;
}

/** Storage for the queue: IndexedDB in the app, an in-memory fake in tests. */
export interface UploadStore {
  put(entry: QueuedUpload): Promise<void>;
  list(): Promise<QueuedUpload[]>;
  remove(sha256: string): Promise<void>;
}

/** The IndexedDB store. Throws on open when IndexedDB is unavailable (private windows). */
export function indexedDbStore(): Promise<UploadStore> {
  return new Promise<UploadStore>((resolve, reject) => {
    const open = indexedDB.open(QUEUE_DB, 1);
    open.onupgradeneeded = () => {
      if (!open.result.objectStoreNames.contains(QUEUE_STORE)) {
        open.result.createObjectStore(QUEUE_STORE, { keyPath: 'sha256' });
      }
    };
    open.onerror = () => reject(open.error ?? new Error('IndexedDB unavailable'));
    open.onsuccess = () => {
      const db = open.result;
      const run = <T>(mode: IDBTransactionMode, work: (store: IDBObjectStore) => IDBRequest<T>) =>
        new Promise<T>((done, fail) => {
          const tx = db.transaction(QUEUE_STORE, mode);
          const request = work(tx.objectStore(QUEUE_STORE));
          tx.oncomplete = () => done(request.result);
          tx.onerror = () => fail(tx.error ?? new Error('queue transaction failed'));
        });
      resolve({
        put: (entry) => run('readwrite', (store) => store.put(entry)).then(() => undefined),
        list: () => run<QueuedUpload[]>('readonly', (store) => store.getAll()),
        remove: (sha256) => run('readwrite', (store) => store.delete(sha256)).then(() => undefined),
      });
    };
  });
}

/** An in-memory store, for tests and for browsers without IndexedDB. */
export function memoryStore(): UploadStore {
  const entries = new Map<string, QueuedUpload>();
  return {
    put: async (entry) => {
      entries.set(entry.sha256, entry);
    },
    list: async () => [...entries.values()],
    remove: async (sha256) => {
      entries.delete(sha256);
    },
  };
}

/** The outcome of one flush of the queue. */
export interface FlushReport {
  uploaded: number;
  duplicates: number;
  extracted: number;
  /** Entries left in the queue (the network failed or the server refused them). */
  pending: number;
  /** Bill ids that were drafted, for the caller to show. */
  billIds: string[];
  failures: string[];
}

/**
 * Uploads every waiting file once, then drafts a bill from it. A network
 * failure stops the flush and leaves the rest queued. A refusal by the server
 * (too large, wrong type) drops that entry and reports it, since retrying can
 * never succeed.
 */
export async function flushUploads(config: ClientConfig, store: UploadStore): Promise<FlushReport> {
  const report: FlushReport = { uploaded: 0, duplicates: 0, extracted: 0, pending: 0, billIds: [], failures: [] };
  const entries = await store.list();
  for (const entry of entries) {
    try {
      const result = await uploadDocument(config, entry.blob, entry.name);
      if (result.duplicate) report.duplicates += 1;
      else report.uploaded += 1;
      try {
        const extraction = await extractBill(config, result.document.id);
        report.extracted += 1;
        report.billIds.push(extraction.bill.id);
      } catch (error) {
        // The bill already exists for this document: nothing more to draft.
        if (!(error instanceof BillApiError && error.status === 409)) throw error;
        if (error.existingBillId) report.billIds.push(error.existingBillId);
      }
      await store.remove(entry.sha256);
    } catch (error) {
      if (error instanceof ApiError && error.kind === 'network') {
        report.pending = entries.length - entries.indexOf(entry);
        return report;
      }
      report.failures.push(`${entry.name}: ${error instanceof Error ? error.message : String(error)}`);
      await store.remove(entry.sha256);
    }
  }
  report.pending = (await store.list()).length;
  return report;
}
