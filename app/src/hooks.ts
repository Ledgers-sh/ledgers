import { useEffect, useState } from 'preact/hooks';
import { approvalsList } from './api/approvals';
import type { ClientConfig } from './api/client';

/** How often the nav badge re-reads the pending count while the app is open. */
const PENDING_POLL_MS = 60_000;

/**
 * The number of approvals waiting for the person. Read on mount, every minute,
 * on window focus, and on `refresh()`. Off (null, no requests) when `enabled` is false.
 */
export function usePendingCount(config: ClientConfig, enabled: boolean): { count: number | null; refresh: () => void } {
  const [count, setCount] = useState<number | null>(null);
  const [tick, setTick] = useState(0);

  useEffect(() => {
    if (!enabled) return;
    let live = true;
    approvalsList(config, 'pending', 1).then(
      (list) => live && setCount(list.pending_count),
      () => live && setCount(null),
    );
    return () => {
      live = false;
    };
  }, [config.baseUrl, config.apiKey, enabled, tick]);

  useEffect(() => {
    if (!enabled) return;
    const bump = () => setTick((t) => t + 1);
    const timer = window.setInterval(bump, PENDING_POLL_MS);
    window.addEventListener('focus', bump);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener('focus', bump);
    };
  }, [enabled]);

  return { count: enabled ? count : null, refresh: () => setTick((t) => t + 1) };
}

export interface AsyncState<T> {
  data: T | undefined;
  error: unknown;
  loading: boolean;
  reload: () => void;
}

/** Runs `load` on mount and whenever `deps` change; `reload` runs it again. */
export function useAsync<T>(load: () => Promise<T>, deps: unknown[]): AsyncState<T> {
  const [state, setState] = useState<{ data: T | undefined; error: unknown; loading: boolean }>({
    data: undefined,
    error: undefined,
    loading: true,
  });
  const [tick, setTick] = useState(0);

  useEffect(() => {
    let alive = true;
    setState((prev) => ({ data: prev.data, error: undefined, loading: true }));
    load().then(
      (data) => alive && setState({ data, error: undefined, loading: false }),
      (error: unknown) => alive && setState((prev) => ({ data: prev.data, error, loading: false })),
    );
    return () => {
      alive = false;
    };
  }, [...deps, tick]);

  return { data: state.data, error: state.error, loading: state.loading, reload: () => setTick((t) => t + 1) };
}

/** The current location hash without the leading "#", e.g. "journal/abc". */
export function useHash(): string {
  const read = () => window.location.hash.replace(/^#\/?/, '');
  const [hash, setHash] = useState(read);
  useEffect(() => {
    const onChange = () => setHash(read());
    window.addEventListener('hashchange', onChange);
    return () => window.removeEventListener('hashchange', onChange);
  }, []);
  return hash;
}

export function useOnline(): boolean {
  const [online, setOnline] = useState(() => (typeof navigator === 'undefined' ? true : navigator.onLine));
  useEffect(() => {
    const up = () => setOnline(true);
    const down = () => setOnline(false);
    window.addEventListener('online', up);
    window.addEventListener('offline', down);
    return () => {
      window.removeEventListener('online', up);
      window.removeEventListener('offline', down);
    };
  }, []);
  return online;
}
