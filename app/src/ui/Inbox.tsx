import { useEffect, useRef, useState } from 'preact/hooks';
import { billsList, type Bill, type BillStatus } from '../api/bills';
import { useAsync } from '../hooks';
import { useSession } from '../session';
import { buildPdf, imageToJpeg, MAX_SCAN_PAGES, needsConversion, quickCheck, sha256Hex, type ScanPage } from '../lib/documents';
import { flushUploads, indexedDbStore, memoryStore, type UploadStore } from '../lib/uploadQueue';
import { Empty, ErrorBanner, Money, PageHead, Skeleton } from './common';
import { t, type MessageKey } from '../i18n';

/** Shown on the Bills screen: upload, scan, drop, the queue and the drafts. */
export function Inbox() {
  const { client, online } = useSession();
  const bills = useAsync(() => billsList(client).then((r) => r.bills), [client.baseUrl, client.apiKey]);
  const storeRef = useRef<UploadStore | null>(null);
  const [queued, setQueued] = useState(0);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<{ kind: 'ok' | 'warn' | 'error'; text: string; href?: string } | null>(null);
  const [scanPages, setScanPages] = useState<ScanPage[]>([]);
  const [dragging, setDragging] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const fileInput = useRef<HTMLInputElement>(null);
  const scanInput = useRef<HTMLInputElement>(null);
  const phone = typeof window !== 'undefined' && window.matchMedia?.('(pointer: coarse)').matches;

  const countQueue = async () => {
    const store = storeRef.current;
    if (store) setQueued((await store.list()).length);
  };

  useEffect(() => {
    let alive = true;
    indexedDbStore()
      .catch(() => memoryStore())
      .then((store) => {
        if (!alive) return;
        storeRef.current = store;
        void countQueue();
      });
    return () => {
      alive = false;
    };
  }, []);

  const flush = async () => {
    const store = storeRef.current;
    if (!store || busy) return;
    setBusy(true);
    setError(null);
    try {
      const report = await flushUploads(client, store);
      const drafted = report.billIds.length;
      if (report.failures.length > 0) {
        setNotice({ kind: 'error', text: t('bills.notice_failed', { count: report.failures.length }) });
      } else if (report.duplicates > 0 && report.uploaded === 0) {
        setNotice({ kind: 'warn', text: t('bills.notice_duplicate'), href: drafted === 1 ? `#/bills/${report.billIds[0]}` : undefined });
      } else if (drafted > 0) {
        setNotice({ kind: 'ok', text: t('bills.notice_drafted', { count: drafted }), href: drafted === 1 ? `#/bills/${report.billIds[0]}` : undefined });
      }
      if (report.pending > 0 && !online) setNotice({ kind: 'warn', text: t('bills.notice_waiting') });
    } catch (err) {
      setError(err);
    } finally {
      setBusy(false);
      await countQueue();
      bills.reload();
    }
  };

  // Files that arrive while online are uploaded at once; offline ones wait for the connection.
  useEffect(() => {
    if (online && queued > 0) void flush();
  }, [online, queued]);

  const enqueue = async (files: File[]) => {
    const store = storeRef.current;
    if (!store || files.length === 0) return;
    for (const file of files) {
      const problem = quickCheck(file);
      if (problem === 'too-large') {
        setNotice({ kind: 'error', text: t('bills.err_too_large') });
        continue;
      }
      if (problem === 'unsupported') {
        setNotice({ kind: 'error', text: t('bills.err_unsupported') });
        continue;
      }
      try {
        let blob: Blob = file;
        let name = file.name;
        if (needsConversion(file)) {
          const page = await imageToJpeg(file);
          blob = new Blob([page.jpeg.slice()], { type: 'image/jpeg' });
          name = name.replace(/\.hei[cf]$/i, '.jpg');
        }
        const sha256 = await sha256Hex(await blob.arrayBuffer());
        await store.put({ sha256, name, type: blob.type, blob, queuedAt: new Date().toISOString() });
      } catch {
        setNotice({ kind: 'error', text: t('bills.err_convert', { name: file.name }) });
      }
    }
    await countQueue();
  };

  const onPicked = (event: Event) => {
    const input = event.target as HTMLInputElement;
    const files = Array.from(input.files ?? []);
    input.value = '';
    void enqueue(files);
  };

  const onDrop = (event: DragEvent) => {
    event.preventDefault();
    setDragging(false);
    void enqueue(Array.from(event.dataTransfer?.files ?? []));
  };

  const addScanPage = async (event: Event) => {
    const input = event.target as HTMLInputElement;
    const file = input.files?.[0];
    input.value = '';
    if (!file) return;
    try {
      const page = await imageToJpeg(file);
      setScanPages((pages) => [...pages.slice(0, MAX_SCAN_PAGES - 1), page]);
    } catch {
      setNotice({ kind: 'error', text: t('bills.err_convert', { name: file.name }) });
    }
  };

  const finishScan = async () => {
    if (scanPages.length === 0) return;
    const pdf = buildPdf(scanPages);
    const stamp = new Date().toISOString().replace(/[:.]/g, '-');
    await enqueue([new File([pdf.slice()], `scan-${stamp}.pdf`, { type: 'application/pdf' })]);
    setScanPages([]);
  };

  const list = bills.data ?? [];
  const pending = list.filter((bill) => bill.status === 'needs-review' || bill.status === 'ready');
  const done = list.filter((bill) => bill.status === 'posted' || bill.status === 'discarded');

  return (
    <>
      <PageHead
        title={t('bills.title')}
        actions={
          <>
            <button type="button" class="btn btn-primary" onClick={() => fileInput.current?.click()}>
              {t('bills.upload')}
            </button>
            {phone && (
              <button type="button" class="btn" onClick={() => scanInput.current?.click()}>
                {t('bills.scan')}
              </button>
            )}
          </>
        }
      >
        {t('bills.intro')}
      </PageHead>

      <input ref={fileInput} type="file" hidden multiple accept="application/pdf,image/png,image/jpeg,image/heic,image/heif" onChange={onPicked} data-testid="upload-input" />
      <input ref={scanInput} type="file" hidden accept="image/*" capture="environment" onChange={addScanPage} data-testid="scan-input" />

      {notice && (
        <div class={`banner ${notice.kind === 'error' ? 'error' : notice.kind === 'warn' ? 'human' : ''}`} role={notice.kind === 'error' ? 'alert' : 'status'}>
          <span>{notice.text}</span>
          {notice.href && (
            <>
              {' '}
              <a href={notice.href}>{t('bills.open_bill')}</a>
            </>
          )}
        </div>
      )}
      {error !== null && <ErrorBanner error={error} />}

      <section
        class={`panel dropzone${dragging ? ' dropzone-active' : ''}`}
        aria-label={t('bills.drop_label')}
        onDragOver={(event) => {
          event.preventDefault();
          setDragging(true);
        }}
        onDragLeave={() => setDragging(false)}
        onDrop={onDrop}
        onKeyDown={(event) => {
          if (event.key === 'Enter' || event.key === ' ') {
            event.preventDefault();
            fileInput.current?.click();
          }
        }}
        tabIndex={0}
        role="button"
      >
        <div class="panel-body dropzone-body">
          <strong>{t('bills.drop_title')}</strong>
          <span class="muted">{t('bills.drop_hint')}</span>
          {queued > 0 && (
            <span class="queue-count" aria-live="polite">
              {t('bills.queued', { count: queued })}
              {!online && ` · ${t('bills.offline_wait')}`}
            </span>
          )}
          {busy && <span class="muted" aria-live="polite">{t('bills.uploading')}</span>}
        </div>
      </section>

      {scanPages.length > 0 && (
        <section class="panel" aria-label={t('bills.scan_section')}>
          <div class="panel-head">
            <h2 class="grow">{t('bills.scan_section')}</h2>
            <span class="muted num">{t('bills.scan_pages', { count: scanPages.length })}</span>
          </div>
          <div class="panel-body row-actions">
            <button type="button" class="btn" onClick={() => scanInput.current?.click()} disabled={scanPages.length >= MAX_SCAN_PAGES}>
              {t('bills.scan_add_page')}
            </button>
            <button type="button" class="btn btn-primary" onClick={() => void finishScan()}>
              {t('bills.scan_finish')}
            </button>
            <button type="button" class="btn btn-ghost" onClick={() => setScanPages([])}>
              {t('bills.scan_cancel')}
            </button>
          </div>
        </section>
      )}

      {bills.loading && bills.data === undefined && <Skeleton rows={4} label={t('bills.loading')} />}
      {bills.error !== undefined && <ErrorBanner error={bills.error} onRetry={bills.reload} />}

      {bills.data !== undefined && list.length === 0 && (
        <Empty title={t('bills.empty_title')}>{t('bills.empty_body')}</Empty>
      )}

      {pending.length > 0 && <BillTable title={t('bills.open_heading')} bills={pending} />}
      {done.length > 0 && <BillTable title={t('bills.done_heading')} bills={done} />}
    </>
  );
}

const STATUS_KEY: Record<BillStatus, MessageKey> = {
  'needs-review': 'bills.status_needs_review',
  ready: 'bills.status_ready',
  posted: 'bills.status_posted',
  discarded: 'bills.status_discarded',
};

function BillTable({ title, bills }: { title: string; bills: Bill[] }) {
  return (
    <section class="panel" aria-label={title}>
      <div class="panel-head">
        <h2 class="grow">{title}</h2>
      </div>
      <div class="table-wrap">
        <table class="table bills-table">
          <thead>
            <tr>
              <th scope="col">{t('bills.col_status')}</th>
              <th scope="col">{t('bills.col_vendor')}</th>
              <th scope="col" class="col-invoice">{t('bills.col_invoice')}</th>
              <th scope="col" class="col-date">{t('bills.col_date')}</th>
              <th scope="col" class="num">{t('bills.col_amount')}</th>
              <th scope="col"><span class="sr-only">{t('bills.col_open')}</span></th>
            </tr>
          </thead>
          <tbody>
            {bills.map((bill) => (
              <tr key={bill.id}>
                <td>
                  <span class={`badge bill-status bill-status-${bill.status}`}>{t(STATUS_KEY[bill.status])}</span>
                </td>
                <td>{bill.vendor.name}</td>
                <td class="mono col-invoice">{bill.invoice_number ?? '—'}</td>
                <td class="mono col-date">{bill.invoice_date ?? '—'}</td>
                <td class="num">
                  {bill.gross_minor !== null ? <Money minor={String(bill.gross_minor)} currency={bill.currency} /> : '—'}
                </td>
                <td>
                  <a href={`#/bills/${encodeURIComponent(bill.id)}`} class="btn btn-sm">
                    {t('bills.review')}
                  </a>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}
