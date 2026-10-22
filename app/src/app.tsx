import { useCallback, useEffect, useState } from 'preact/hooks';
import { useHash, useOnline } from './hooks';
import {
  clearCredentials,
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
import { Icon, type IconName } from './ui/icons';
import { Toasts, useToasts } from './ui/common';
import { Overview } from './ui/Overview';
import { Accounts } from './ui/Accounts';
import { Journal, EntryDetail } from './ui/Journal';
import { NewEntry } from './ui/NewEntry';
import { Reports } from './ui/Reports';
import { Bills } from './ui/Bills';

const NAV: { hash: string; label: string; icon: IconName; match: (route: string) => boolean }[] = [
  { hash: 'overview', label: 'Overview', icon: 'overview', match: (r) => r === '' || r === 'overview' },
  { hash: 'journal', label: 'Journal', icon: 'journal', match: (r) => r === 'journal' || r.startsWith('journal/') },
  { hash: 'accounts', label: 'Accounts', icon: 'accounts', match: (r) => r === 'accounts' },
  { hash: 'reports', label: 'Reports', icon: 'reports', match: (r) => r === 'reports' },
  { hash: 'bills', label: 'Bills', icon: 'bills', match: (r) => r === 'bills' },
];

export function App() {
  const [credentials, setCredentials] = useState<Credentials | null>(() => loadCredentials());

  if (!credentials) {
    return (
      <Connect
        onConnected={(next) => {
          saveCredentials(next);
          setCredentials(next);
        }}
      />
    );
  }
  return (
    <Shell
      credentials={credentials}
      onSignOut={() => {
        clearCredentials();
        void clearApiCaches();
        setCredentials(null);
      }}
    />
  );
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

function Shell({ credentials, onSignOut }: { credentials: Credentials; onSignOut: () => void }) {
  const hash = useHash();
  const online = useOnline();
  const { items, push, dismiss } = useToasts();
  const [period, setPeriodState] = useState<string>(() => loadPeriod());
  const [currency, setCurrencyState] = useState<string>(() => loadCurrency());
  const [theme, setTheme] = useState<Theme>(() => loadTheme());

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    saveTheme(theme);
  }, [theme]);

  const setPeriod = useCallback((next: string) => {
    savePeriod(next);
    setPeriodState(next);
  }, []);
  const setCurrency = useCallback((next: string) => {
    saveCurrency(next);
    setCurrencyState(next);
  }, []);

  const session: Session = {
    client: { baseUrl: credentials.baseUrl, apiKey: credentials.apiKey },
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
  const page = renderRoute(route);
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
            {NAV.map((item) => (
              <a key={item.hash} href={`#/${item.hash}`} aria-current={current === item ? 'page' : undefined}>
                <Icon name={item.icon} />
                {item.label}
              </a>
            ))}
          </nav>
          <div class="sidebar-foot">
            <span>
              Server <code>{credentials.baseUrl.replace(/^https?:\/\//, '')}</code>
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
            <span class="ledger-id" title={credentials.baseUrl}>
              {credentials.baseUrl.replace(/^https?:\/\//, '')}
            </span>
            <div class="grow" />
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
        {NAV.map((item) => (
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

function renderRoute(route: string) {
  if (route === '' || route === 'overview') return <Overview />;
  if (route === 'journal') return <Journal />;
  if (route.startsWith('journal/')) return <EntryDetail entryId={decodeURIComponent(route.slice('journal/'.length))} />;
  if (route === 'accounts') return <Accounts />;
  if (route === 'reports') return <Reports />;
  if (route === 'bills') return <Bills />;
  if (route === 'new') return <NewEntry />;
  return (
    <div class="empty">
      <h2>Page not found</h2>
      <p>
        <a href="#/overview">Go to the overview</a>
      </p>
    </div>
  );
}
