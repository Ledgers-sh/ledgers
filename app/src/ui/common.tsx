import { useEffect, useRef, useState } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import { ApiError, type ApiErrorKind } from '../api/client';
import { formatMinor } from '../lib/money';
import { intlLocale, t, type MessageKey } from '../i18n';
import { Ltr } from '../i18n/Ltr';

/** An amount in its currency's own decimals, right aligned. Digits stay left-to-right in every language. */
export function Money({ minor, currency, class: className }: { minor: string | bigint; currency: string; class?: string }) {
  const negative = String(minor).startsWith('-');
  return (
    <span class={`num${negative ? ' neg' : ''} ${className ?? ''}`.trim()}>
      <Ltr>{formatMinor(minor, currency, intlLocale())}</Ltr>
      <span class="sr-only"> {currency}</span>
    </span>
  );
}

const API_KIND_KEYS: Record<ApiErrorKind, MessageKey> = {
  unauthorized: 'error.unauthorized',
  forbidden: 'error.forbidden',
  validation: 'error.validation',
  not_found: 'error.not_found',
  unavailable: 'error.unavailable',
  server: 'error.server',
  network: 'error.network',
};

export function describeError(error: unknown): { title: string; detail: string | null } {
  if (error instanceof ApiError) {
    const title = t(API_KIND_KEYS[error.kind]);
    // A validation message is the server's own words; the detail adds the specifics.
    const detail = error.detail && error.detail !== title ? error.detail : null;
    return { title, detail };
  }
  if (error instanceof Error) return { title: error.message, detail: null };
  return { title: t('common.something_wrong'), detail: null };
}

export function ErrorBanner({ error, onRetry }: { error: unknown; onRetry?: () => void }) {
  const { title, detail } = describeError(error);
  return (
    <div class="banner error" role="alert">
      <div class="grow">
        <strong>{title}</strong>
        {detail && <div class="muted mono" style={{ marginTop: '4px' }}>{detail}</div>}
      </div>
      {onRetry && (
        <button type="button" class="btn btn-sm" onClick={onRetry}>
          {t('common.try_again')}
        </button>
      )}
    </div>
  );
}

export function Skeleton({ rows = 4, label }: { rows?: number; label?: string }) {
  return (
    <div class="panel" role="status" aria-live="polite">
      <span class="sr-only">{label ?? t('common.loading')}</span>
      <div class="skeleton" aria-hidden="true">
        {Array.from({ length: rows }, (_, i) => (
          <i key={i} style={{ width: `${92 - ((i * 13) % 35)}%` }} />
        ))}
      </div>
    </div>
  );
}

export function Empty({ title, children, action }: { title: string; children?: ComponentChildren; action?: ComponentChildren }) {
  return (
    <div class="empty">
      <h2>{title}</h2>
      {children && <p>{children}</p>}
      {action}
    </div>
  );
}

export function PageHead({ title, children, actions }: { title: string; children?: ComponentChildren; actions?: ComponentChildren }) {
  return (
    <header class="page-head">
      <div>
        <h1>{title}</h1>
        {children && <p>{children}</p>}
      </div>
      {actions && <div class="row-actions">{actions}</div>}
    </header>
  );
}

export function Marker({ by }: { by: 'agent' | 'person' }) {
  return <span class={`marker ${by === 'agent' ? 'agent' : 'human'}`}>{by === 'agent' ? t('marker.agent') : t('marker.person')}</span>;
}

/** Native <dialog> confirm: Esc and the backdrop close it, focus returns to the opener. */
export function ConfirmDialog({
  open,
  onClose,
  title,
  children,
  confirmLabel,
  onConfirm,
  busy,
  error,
  danger,
  disabled,
  tone,
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  children: ComponentChildren;
  confirmLabel: string;
  onConfirm: () => void;
  busy?: boolean;
  error?: unknown;
  danger?: boolean;
  disabled?: boolean;
  /** "human" gives the confirm button the human-action gold (approvals). */
  tone?: 'human';
}) {
  const ref = useRef<HTMLDialogElement>(null);
  // Each dialog names itself with its own heading id: two dialogs on one page must not share an id.
  const [titleId] = useState(() => `confirm-title-${Math.random().toString(36).slice(2, 9)}`);
  useEffect(() => {
    const dialog = ref.current;
    if (!dialog) return;
    if (open && !dialog.open) dialog.showModal();
    if (!open && dialog.open) dialog.close();
  }, [open]);
  return (
    <dialog
      ref={ref}
      class="confirm"
      aria-labelledby={titleId}
      onClose={onClose}
      onCancel={(event) => {
        event.preventDefault();
        if (!busy) onClose();
      }}
    >
      <div class="dialog-body">
        <h2 id={titleId}>{title}</h2>
        {children}
        {error !== undefined && error !== null && <ErrorBanner error={error} />}
        <div class="dialog-actions">
          <button type="button" class="btn" onClick={onClose} disabled={busy}>
            {t('common.cancel')}
          </button>
          <button
            type="button"
            class={`btn ${danger ? 'btn-danger' : tone === 'human' ? 'btn-human' : 'btn-primary'}`}
            onClick={onConfirm}
            disabled={busy || disabled}
            aria-busy={busy ? 'true' : undefined}
          >
            {busy ? t('common.working') : confirmLabel}
          </button>
        </div>
      </div>
    </dialog>
  );
}

export interface ToastItem {
  id: number;
  message: string;
  kind: 'ok' | 'error';
}

export function Toasts({ items, dismiss }: { items: ToastItem[]; dismiss: (id: number) => void }) {
  useEffect(() => {
    if (items.length === 0) return;
    const timer = window.setTimeout(() => dismiss(items[0].id), 4500);
    return () => window.clearTimeout(timer);
  }, [items, dismiss]);
  return (
    <div class="toasts" role="status" aria-live="polite">
      {items.map((item) => (
        <div key={item.id} class={`toast ${item.kind === 'error' ? 'error' : ''}`}>
          <span>{item.message}</span>
          <button type="button" class="btn btn-ghost btn-sm" onClick={() => dismiss(item.id)} aria-label={t('common.dismiss')}>
            ×
          </button>
        </div>
      ))}
    </div>
  );
}

export function useToasts() {
  const [items, setItems] = useState<ToastItem[]>([]);
  const counter = useRef(0);
  const push = (message: string, kind: 'ok' | 'error' = 'ok') => {
    counter.current += 1;
    const id = counter.current;
    setItems((prev) => [...prev, { id, message, kind }]);
  };
  const dismiss = (id: number) => setItems((prev) => prev.filter((item) => item.id !== id));
  return { items, push, dismiss };
}
