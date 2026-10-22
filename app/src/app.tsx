import { useCallback, useEffect, useState } from 'preact/hooks';
import { useHash, useOnline } from './hooks';
import { BILLS_MATCH_OPERATION, fetchOperationIds } from './api/features';
import { ApiError } from './api/client';
import { fetchSession, initialOf, signOut, type Person } from './api/auth';
import {
  clearCredentials,
  DEFAULT_SERVER_URL,
  loadCredentials,
  loadCurrency,
  loadPeriod,
  loadTheme,
  saveCredentials,
  saveCurrency,
  savePeriod,
  saveTheme,
  SessionContext,
  type Credentials,
  type Session,
  type Theme,
  type ToastKind,
} from './session';
import { Connect } from './ui/Connect';
import { SignIn } from './ui/SignIn';
import { Icon, type IconName } from './ui/icons';
import { Toasts, useToasts } from './ui/common';
import { Overview } from './ui/Overview';
import { Accounts } from './ui/Accounts';
import { Journal, EntryDetail } from './ui/Journal';
import { NewEntry } from './ui/NewEntry';
import { Reports } from './ui/Reports';
import { Bills } from './ui/Bills';
import { Settings } from './ui/Settings';

const NAV: { hash: string; label: string; icon: IconName; match: (route: string) => boolean }[] = [
  { hash: 'overview', label: 'Overview', icon: 'overview', match: (r) => r === '' || r === 'overview' },
  { hash: 'journal', label: 'Journal', icon: 'journal', match: (r) => r === 'journal' || r.startsWith('journal/') },
  { hash: 'accounts', label: 'Accounts', icon: 'accounts', match: (r) => r === 'accounts' },
  { hash: 'reports', label: 'Reports', icon: 'reports', match: (r) => r === 'reports' },
  { hash: 'bills', label: 'Bills', icon: 'bills', match: (r) => r === 'bills' },
  { hash: 'settings/agent-keys', label: 'Settings', icon: 'settings', match: (r) => r === 'settings' || r.startsWith('settings/') },
];

/**
 * Bills is shown only when the server's OpenAPI document lists bills.match.
 * `checking` covers the first moments after connecting, before the document arrives.
 */
type BillsState = 'checking' | 'offered' | 'absent';

/**
 * Who is using the app. A person signs in with a passkey on the server and
 * their session cookie is sent with each call. An agent key (advanced, for
 * self-hosted servers and automation) is kept in session storage instead.
 */
type Mode =
  | { kind: 'checking' }
  | { kind: 'signed-out'; error: string | null; advanced: boolean }
  | { kind: 'forbidden'; detail: string }
  | { kind: 'person'; baseUrl: string; person: Person }
  | { kind: 'key'; credentials: Credentials };

export function App() {
  const [mode, setMode] = useState<Mode>(() => {
    const stored = loadCredentials();
    return stored ? { kind: 'key', credentials: stored } : { kind: 'checking' };
  });

  useEffect(() => {
    if (mode.kind !== 'checking') return;
    let live = true;
    fetchSession(DEFAULT_SERVER_URL).then(
      (person) => {
        if (!live) return;
        if (person) setMode({ kind: 'person', baseUrl: DEFAULT_SERVER_URL, person });
        else setMode({ kind: 'signed-out', error: null, advanced: false });
      },
      (error: unknown) => {
        if (!live) return;
        if (error instanceof ApiError && error.kind === 'forbidden') {
          setMode({ kind: 'forbidden', detail: error.detail ?? error.message });
        } else {
          const message = error instanceof ApiError ? (error.detail ?? error.message) : 'Sign-in is not available right now.';
          setMode({ kind: 'signed-out', error: message, advanced: false });
        }
      },
    );
    return () => {
      live = false;
    };
  }, [mode.kind]);

  const endPersonSession = useCallback(async (baseUrl: string) => {
    await signOut(baseUrl);
    void clearApiCaches();
    setMode({ kind: 'signed-out', error: null, advanced: false });
  }, []);

  switch (mode.kind) {
    case 'checking':
      return (
        <main class="connect" id="main">
          <p class="muted" role="status">Checking your sign-in…</p>
        </main>
      );
    case 'signed-out':
      return mode.advanced ? (
        <Connect
          onBack={() => setMode({ kind: 'signed-out', error: null, advanced: false })}
          onConnected={(next) => {
            saveCredentials(next);
            setMode({ kind: 'key', credentials: next });
          }}
        />
      ) : (
        <SignIn
          baseUrl={DEFAULT_SERVER_URL}
          error={mode.error}
          onAdvanced={() => setMode({ kind: 'signed-out', error: null, advanced: true })}
        />
      );
    case 'forbidden':
      return (
        <main class="connect" id="main">
          <div class="connect-card" role="alert">
            <h1 style={{ font: 'inherit' }}>This account cannot use this ledger</h1>
            <p class="muted">{mode.detail}</p>
            <div class="row-actions">
              <button
                type="button"
                class="btn btn-primary"
                onClick={() => void endPersonSession(DEFAULT_SERVER_URL)}
              >
                Sign out
              </button>
            </div>
          </div>
        </main>
      );
    case 'person':
      return (
        <Shell
          baseUrl={mode.baseUrl}
          person={mode.person}
          onSignOut={() => void endPersonSession(mode.baseUrl)}
        />
      );
    case 'key':
      return (
        <Shell
          baseUrl={mode.credentials.baseUrl}
          apiKey={mode.credentials.apiKey}
          onSignOut={() => {
            clearCredentials();
            void clearApiCaches();
            setMode({ kind: 'signed-out', error: null, advanced: false });
          }}
        />
      );
  }
}

/** Drops cached read views on sign-out so the next person on this device sees nothing. */
async function clearApiCaches(): Promise<void> {
  try {
    if (!('caches' in window)) return;
    for (const name of await caches.keys()) {
      if (name.startsWith('ledgers-api-')) await caches.delete(name);
    }
  } catch {
    // Cache API unavailable; nothing to clear.
  }
}

function Shell({
  baseUrl,
  apiKey,
  person,
  onSignOut,
}: {
  baseUrl: string;
  apiKey?: string;
  person?: Person;
  onSignOut: () => void;
}) {
  const hash = useHash();
  const online = useOnline();
  const { items, push, dismiss } = useToasts();
  const [period, setPeriodState] = useState<string>(() => loadPeriod());
  const [currency, setCurrencyState] = useState<string>(() => loadCurrency());
  const [theme, setTheme] = useState<Theme>(() => loadTheme());
  const [billsState, setBillsState] = useState<BillsState>('checking');

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    saveTheme(theme);
  }, [theme]);

  useEffect(() => {
    let live = true;
    setBillsState('checking');
    void fetchOperationIds(baseUrl).then((ids) => {
      if (live) setBillsState(ids.has(BILLS_MATCH_OPERATION) ? 'offered' : 'absent');
    });
    return () => {
      live = false;
    };
  }, [baseUrl]);

  const nav = NAV.filter((item) => item.hash !== 'bills' || billsState === 'offered');

  const setPeriod = useCallback((next: string) => {
    savePeriod(next);
    setPeriodState(next);
  }, []);
  const setCurrency = useCallback((next: string) => {
    saveCurrency(next);
    setCurrencyState(next);
  }, []);

  const session: Session = {
    client: apiKey ? { baseUrl, apiKey } : { baseUrl },
    period,
    setPeriod,
    currency,
    setCurrency,
    online,
    toast: (message: string, kind: ToastKind = 'ok') => push(message, kind),
    signOut: onSignOut,
  };

  // Keyboard shortcuts: N opens New entry, / jumps to the journal search. Ignored while typing.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.metaKey || event.ctrlKey || event.altKey) return;
      const target = event.target as HTMLElement | null;
      if (target && (target.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName))) return;
      if (document.querySelector('dialog[open]')) return;
      if (event.key === 'n' || event.key === 'N') {
        event.preventDefault();
        window.location.hash = '#/new';
      } else if (event.key === '/') {
        event.preventDefault();
        window.location.hash = '#/journal';
        window.setTimeout(() => document.getElementById('journal-search')?.focus(), 0);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  const route = hash.replace(/\?.*$/, '');
  const page = renderRoute(route, billsState);
  const current = NAV.find((item) => item.match(route));

  return (
    <SessionContext.Provider value={session}>
      <a class="skip" href="#main">
        Skip to content
      </a>
      <div class="app">
        <aside class="sidebar" aria-label="Main">
          <a class="brand" href="#/overview" aria-label="ledgers.sh overview">
            <img src="/assets/favicon.svg" alt="" width="28" height="28" />
            <span>ledgers<span>.sh</span></span>
          </a>
          <nav class="nav" aria-label="Sections">
            {nav.map((item) => (
              <a key={item.hash} href={`#/${item.hash}`} aria-current={current === item ? 'page' : undefined}>
                <Icon name={item.icon} />
                {item.label}
              </a>
            ))}
          </nav>
          <div class="sidebar-foot">
            <span>
              Server <code>{baseUrl.replace(/^https?:\/\//, '')}</code>
            </span>
            <button type="button" class="btn btn-ghost btn-sm" onClick={onSignOut} style={{ justifyContent: 'flex-start' }}>
              <Icon name="logout" />
              Sign out
            </button>
          </div>
        </aside>
        <div class="main">
          <header class="topbar">
            <a class="mobile-brand" href="#/overview">
              <img src="/assets/favicon.svg" alt="" width="24" height="24" />
              <span>ledgers<span class="muted">.sh</span></span>
            </a>
            <span class="ledger-id" title={baseUrl}>
              {baseUrl.replace(/^https?:\/\//, '')}
            </span>
            <div class="grow" />
            {person && (
              <span class="person" title={person.email ?? person.personId}>
                <span class="avatar" aria-hidden="true">
                  {initialOf(person)}
                </span>
                <span class="person-name">{person.email ?? 'Signed in'}</span>
              </span>
            )}
            <label class="period">
              <span>Period</span>
              <input
                class="input"
                type="month"
                value={period}
                onInput={(event) => {
                  const value = (event.target as HTMLInputElement).value;
                  if (/^\d{4}-\d{2}$/.test(value)) setPeriod(value);
                }}
              />
            </label>
            <span class="kbd-hint" aria-hidden="true">
              <kbd>N</kbd> new <kbd>/</kbd> search
            </span>
            <a class="btn btn-sm btn-primary" href="#/new" aria-keyshortcuts="N">
              <Icon name="plus" />
              <span class="btn-label">New entry</span>
            </a>
            <button
              type="button"
              class="btn btn-ghost btn-sm"
              onClick={() => setTheme(theme === 'dark' ? 'light' : 'dark')}
              aria-label={theme === 'dark' ? 'Switch to light mode' : 'Switch to dark mode'}
              title={theme === 'dark' ? 'Light mode' : 'Dark mode'}
            >
              <Icon name={theme === 'dark' ? 'sun' : 'moon'} />
            </button>
          </header>
          {!online && (
            <div class="banner human" role="status" style={{ margin: '12px 24px 0' }}>
              <span class="grow">
                <strong>Offline.</strong> Read views show the last data loaded. Posting, reversing and matching need the server.
              </span>
            </div>
          )}
          <main id="main" class="content">
            {page}
          </main>
        </div>
      </div>
      <nav class="bottom-nav" aria-label="Sections">
        {nav.map((item) => (
          <a key={item.hash} href={`#/${item.hash}`} aria-current={current === item ? 'page' : undefined}>
            <Icon name={item.icon} />
            {item.label}
          </a>
        ))}
      </nav>
      <Toasts items={items} dismiss={dismiss} />
    </SessionContext.Provider>
  );
}

function renderRoute(route: string, billsState: BillsState) {
  if (route === '' || route === 'overview') return <Overview />;
  if (route === 'journal') return <Journal />;
  if (route.startsWith('journal/')) return <EntryDetail entryId={decodeURIComponent(route.slice('journal/'.length))} />;
  if (route === 'accounts') return <Accounts />;
  if (route === 'reports') return <Reports />;
  if (route === 'bills' && billsState === 'offered') return <Bills />;
  if (route === 'bills' && billsState === 'checking') return <p class="muted">Checking what this server offers…</p>;
  if (route === 'new') return <NewEntry />;
  if (route === 'settings' || route === 'settings/agent-keys') return <Settings section="agent-keys" />;
  return (
    <div class="empty">
      <h2>Page not found</h2>
      <p>
        <a href="#/overview">Go to the overview</a>
      </p>
    </div>
  );
}
