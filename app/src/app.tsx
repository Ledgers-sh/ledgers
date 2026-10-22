import { useCallback, useEffect, useMemo, useState } from 'preact/hooks';
import { useHash, useOnline, usePendingCount } from './hooks';
import { BILLS_MATCH_OPERATION, operationIds } from './api/features';
import { fetchOpenApiDocument } from './api/openapi';
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
import { describeError, Toasts, useToasts } from './ui/common';
import { Overview } from './ui/Overview';
import { Accounts } from './ui/Accounts';
import { Journal, EntryDetail } from './ui/Journal';
import { NewEntry } from './ui/NewEntry';
import { Reports } from './ui/Reports';
import { Bills } from './ui/Bills';
import { ApprovalDetail, Approvals } from './ui/Approvals';
import { badgeText } from './lib/approvals';
import { t, useLocale, type MessageKey } from './i18n';
import { LanguagePicker } from './i18n/LanguagePicker';
import { Ltr } from './i18n/Ltr';
import { CommandPalette, type PaletteData } from './palette/CommandPalette';
import { paletteCommands, type Session as PaletteSession } from './palette/model';

const NAV: { hash: string; label: MessageKey; icon: IconName; match: (route: string) => boolean }[] = [
  { hash: 'overview', label: 'nav.overview', icon: 'overview', match: (r) => r === '' || r === 'overview' },
  { hash: 'journal', label: 'nav.journal', icon: 'journal', match: (r) => r === 'journal' || r.startsWith('journal/') },
  { hash: 'accounts', label: 'nav.accounts', icon: 'accounts', match: (r) => r === 'accounts' },
  { hash: 'reports', label: 'nav.reports', icon: 'reports', match: (r) => r === 'reports' },
  { hash: 'bills', label: 'nav.bills', icon: 'bills', match: (r) => r === 'bills' },
  { hash: 'approvals', label: 'nav.approvals', icon: 'approvals', match: (r) => r === 'approvals' || r.startsWith('approvals/') },
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
  // Re-render the whole tree when the language changes.
  useLocale();
  // The sign-in screens take the saved theme too (the shell applies it once signed in).
  useEffect(() => {
    document.documentElement.dataset.theme = loadTheme();
  }, []);
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
          const message = error instanceof ApiError ? (error.detail ?? describeError(error).title) : t('signin.unavailable');
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
          <p class="muted" role="status">
            {t('shell.checking_sign_in')}
          </p>
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
            <h1 style={{ font: 'inherit' }}>{t('forbidden.title')}</h1>
            <p class="muted">{mode.detail}</p>
            <div class="row-actions">
              <button
                type="button"
                class="btn btn-primary"
                onClick={() => void endPersonSession(DEFAULT_SERVER_URL)}
              >
                {t('forbidden.sign_out')}
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
  const locale = useLocale();
  const { items, push, dismiss } = useToasts();
  const [period, setPeriodState] = useState<string>(() => loadPeriod());
  const [currency, setCurrencyState] = useState<string>(() => loadCurrency());
  const [theme, setTheme] = useState<Theme>(() => loadTheme());
  // The OpenAPI document: undefined while loading, null when the server did not return one.
  const [openApi, setOpenApi] = useState<unknown | null | undefined>(undefined);
  const [paletteOpen, setPaletteOpen] = useState(false);
  // Approvals are for a signed-in person; an agent key gets 403 there, so the nav and badge stay off.
  const approvals = usePendingCount({ baseUrl }, !apiKey);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    saveTheme(theme);
  }, [theme]);

  useEffect(() => {
    let live = true;
    setOpenApi(undefined);
    void fetchOpenApiDocument(baseUrl).then((doc) => {
      if (live) setOpenApi(doc);
    });
    return () => {
      live = false;
    };
  }, [baseUrl]);

  const billsState: BillsState =
    openApi === undefined ? 'checking' : operationIds(openApi).has(BILLS_MATCH_OPERATION) ? 'offered' : 'absent';

  const paletteData: PaletteData = useMemo(() => {
    if (openApi === undefined) return { status: 'loading' };
    if (openApi === null) return { status: 'unavailable' };
    return { status: 'ready', commands: paletteCommands(openApi) };
  }, [openApi]);

  const nav = NAV.filter((item) => (item.hash !== 'bills' || billsState === 'offered') && (item.hash !== 'approvals' || !apiKey));
  const navBadge = (hash: string) => (hash === 'approvals' ? badgeText(approvals.count) : null);

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
    approvalCount: approvals.count,
    refreshApprovals: approvals.refresh,
  };

  // The session the palette judges access by: a person's roles, or an agent key (the server applies its tier).
  const paletteSession: PaletteSession = person ? { kind: 'person', roles: person.roles } : { kind: 'key' };

  // ⌘K / Ctrl+K opens the command palette from anywhere in the app.
  useEffect(() => {
    const onCommandKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && !event.altKey && event.key.toLowerCase() === 'k') {
        event.preventDefault();
        setPaletteOpen(true);
      }
    };
    window.addEventListener('keydown', onCommandKey);
    return () => window.removeEventListener('keydown', onCommandKey);
  }, []);

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
  const shortcutKey = /Mac|iPhone|iPad/.test(navigator.platform) ? '⌘K' : 'Ctrl K';

  return (
    <SessionContext.Provider value={session}>
      <a class="skip" href="#main">
        {t('shell.skip')}
      </a>
      <div class="app">
        <aside class="sidebar" aria-label={t('shell.main_nav')}>
          <a class="brand" href="#/overview" aria-label={t('shell.brand_link')}>
            <img src="/assets/favicon.svg" alt="" width="28" height="28" />
            <span>ledgers<span>.sh</span></span>
          </a>
          <nav class="nav" aria-label={t('shell.sections')}>
            {nav.map((item) => (
              <a key={item.hash} href={`#/${item.hash}`} aria-current={current === item ? 'page' : undefined}>
                <Icon name={item.icon} />
                {t(item.label)}
                {navBadge(item.hash) && <span class="nav-count">{navBadge(item.hash)}</span>}
              </a>
            ))}
          </nav>
          <div class="sidebar-foot">
            <span>
              {t('shell.server')} <code><Ltr>{baseUrl.replace(/^https?:\/\//, '')}</Ltr></code>
            </span>
            <button type="button" class="btn btn-ghost btn-sm" onClick={onSignOut} style={{ justifyContent: 'flex-start' }}>
              <Icon name="logout" />
              {t('shell.sign_out')}
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
              <Ltr>{baseUrl.replace(/^https?:\/\//, '')}</Ltr>
            </span>
            <div class="grow" />
            {person && (
              <span class="person" title={person.email ?? person.personId}>
                <span class="avatar" aria-hidden="true">
                  {initialOf(person)}
                </span>
                <span class="person-name">{person.email ?? t('shell.signed_in')}</span>
              </span>
            )}
            <label class="period">
              <span>{t('shell.period')}</span>
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
              <kbd>N</kbd> {t('shell.hint_new')} <kbd>/</kbd> {t('shell.hint_search')}
            </span>
            <button
              type="button"
              class="btn btn-sm btn-ghost palette-button"
              aria-haspopup="dialog"
              aria-keyshortcuts="Control+K Meta+K"
              title={`${t('palette.button')} (${shortcutKey})`}
              onClick={() => setPaletteOpen(true)}
            >
              <span>{t('palette.button')}</span>
              <kbd aria-hidden="true">{shortcutKey}</kbd>
            </button>
            <LanguagePicker />
            <a class="btn btn-sm btn-primary" href="#/new" aria-keyshortcuts="N">
              <Icon name="plus" />
              <span class="btn-label">{t('shell.new_entry')}</span>
            </a>
            <button
              type="button"
              class="btn btn-ghost btn-sm"
              onClick={() => setTheme(theme === 'dark' ? 'light' : 'dark')}
              aria-label={theme === 'dark' ? t('shell.theme_to_light') : t('shell.theme_to_dark')}
              title={theme === 'dark' ? t('shell.theme_title_light') : t('shell.theme_title_dark')}
            >
              <Icon name={theme === 'dark' ? 'sun' : 'moon'} />
            </button>
          </header>
          {!online && (
            <div class="banner human" role="status" style={{ margin: '12px 24px 0' }}>
              <span class="grow">
                <strong>{t('shell.offline_title')}</strong> {t('shell.offline_body')}
              </span>
            </div>
          )}
          <main id="main" class="content" lang={locale.code} dir={locale.dir}>
            {page}
          </main>
        </div>
      </div>
      <nav class="bottom-nav" aria-label={t('shell.sections')}>
        {nav.map((item) => (
          <a key={item.hash} href={`#/${item.hash}`} aria-current={current === item ? 'page' : undefined}>
            <Icon name={item.icon} />
            {t(item.label)}
            {navBadge(item.hash) && <span class="nav-count">{navBadge(item.hash)}</span>}
          </a>
        ))}
      </nav>
      <CommandPalette
        open={paletteOpen}
        onClose={() => setPaletteOpen(false)}
        data={paletteData}
        session={paletteSession}
        onOpenHash={(next) => {
          setPaletteOpen(false);
          window.location.hash = next;
        }}
      />
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
  if (route === 'approvals') return <Approvals />;
  if (route.startsWith('approvals/')) return <ApprovalDetail approvalId={decodeURIComponent(route.slice('approvals/'.length))} />;
  if (route === 'bills' && billsState === 'offered') return <Bills />;
  if (route === 'bills' && billsState === 'checking') return <p class="muted">{t('shell.checking_bills')}</p>;
  if (route === 'new') return <NewEntry />;
  return (
    <div class="empty">
      <h2>{t('shell.not_found')}</h2>
      <p>
        <a href="#/overview">{t('shell.go_overview')}</a>
      </p>
    </div>
  );
}
