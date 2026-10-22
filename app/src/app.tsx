import { useCallback, useEffect, useMemo, useState } from 'preact/hooks';
import { useHash, useOnline, usePendingCount } from './hooks';
import { fetchOpenApiDocument } from './api/openapi';
import { ApiError } from './api/client';
import { fetchSession, initialOf, NotAMemberError, signOut, type Person } from './api/auth';
import { isAcceptPath } from './lib/people';
import { AcceptInvitation } from './ui/Accept';
import { NotMember } from './ui/NotMember';
import {
  clearCredentials,
  DEFAULT_SERVER_URL,
  loadCredentials,
  loadCurrency,
  loadRange,
  loadTheme,
  saveCredentials,
  saveCurrency,
  savePeriod,
  saveRange,
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
import { AccountNew } from './ui/AccountNew';
import { AccountDetail } from './ui/AccountDetail';
import { Journal, EntryDetail } from './ui/Journal';
import { NewEntry } from './ui/NewEntry';
import { Reports } from './ui/Reports';
import { Inbox } from './ui/Inbox';
import { BillReview } from './ui/BillReview';
import { BankAccounts, BankAccountNew } from './ui/BankAccounts';
import { BankAccountPage } from './ui/BankAccount';
import { BankRules } from './ui/BankRules';
import { ApprovalDetail, Approvals } from './ui/Approvals';
import { Settings, settingsSectionOf } from './ui/Settings';
import { badgeText } from './lib/approvals';
import { normaliseRange, type MonthRange } from './lib/trends';
import { PeriodPicker } from './ui/PeriodPicker';
import { t, useLocale, type MessageKey } from './i18n';

import { LanguagePicker } from './i18n/LanguagePicker';
import { Ltr } from './i18n/Ltr';
import { CommandPalette, type PaletteData } from './palette/CommandPalette';
import { paletteCommands, type Session as PaletteSession } from './palette/model';

const NAV: { hash: string; label: MessageKey; icon: IconName; match: (route: string) => boolean }[] = [
  { hash: 'overview', label: 'nav.overview', icon: 'overview', match: (r) => r === '' || r === 'overview' },
  { hash: 'journal', label: 'nav.journal', icon: 'journal', match: (r) => r === 'journal' || r.startsWith('journal/') },
  { hash: 'accounts', label: 'nav.accounts', icon: 'accounts', match: (r) => r === 'accounts' || r.startsWith('accounts/') },
  { hash: 'reports', label: 'nav.reports', icon: 'reports', match: (r) => r === 'reports' },
  { hash: 'bills', label: 'nav.bills', icon: 'bills', match: (r) => r === 'bills' || r.startsWith('bills/') },
  { hash: 'bank', label: 'nav.bank', icon: 'bank', match: (r) => r === 'bank' || r.startsWith('bank/') },
  { hash: 'settings', label: 'nav.settings', icon: 'settings', match: (r) => r === 'settings' || r.startsWith('settings/') },
  { hash: 'approvals', label: 'nav.approvals', icon: 'approvals', match: (r) => r === 'approvals' || r.startsWith('approvals/') },
];

/**
 * Who is using the app. A person signs in with a passkey on the server and
 * their session cookie is sent with each call. An agent key (advanced, for
 * self-hosted servers and automation) is kept in session storage instead.
 */
type Mode =
  | { kind: 'checking' }
  | { kind: 'signed-out'; error: string | null; advanced: boolean }
  | { kind: 'forbidden'; detail: string }
  | { kind: 'not-member'; title: string }
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
    // The accept page checks the sign-in itself (see AcceptInvitation).
    if (mode.kind !== 'checking' || isAcceptPath(window.location.pathname)) return;
    let live = true;
    fetchSession(DEFAULT_SERVER_URL).then(
      (person) => {
        if (!live) return;
        if (person) setMode({ kind: 'person', baseUrl: DEFAULT_SERVER_URL, person });
        else setMode({ kind: 'signed-out', error: null, advanced: false });
      },
      (error: unknown) => {
        if (!live) return;
        if (error instanceof NotAMemberError) {
          setMode({ kind: 'not-member', title: error.title });
        } else if (error instanceof ApiError && error.kind === 'forbidden') {
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

  // The invitation link: reachable signed in or not; the page signs the person in when it must.
  if (isAcceptPath(window.location.pathname)) return <AcceptInvitation />;

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
    case 'not-member':
      return (
        <NotMember title={mode.title} onSignOut={() => void endPersonSession(DEFAULT_SERVER_URL)} />
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
  const [range, setRangeState] = useState<MonthRange>(() => loadRange());
  const period = range.to;
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

  const paletteData: PaletteData = useMemo(() => {
    if (openApi === undefined) return { status: 'loading' };
    if (openApi === null) return { status: 'unavailable' };
    return { status: 'ready', commands: paletteCommands(openApi) };
  }, [openApi]);

  const nav = NAV.filter((item) => item.hash !== 'approvals' || !apiKey);
  const navBadge = (hash: string) => (hash === 'approvals' ? badgeText(approvals.count) : null);

  const setRange = useCallback((next: MonthRange) => {
    const normalised = normaliseRange(next);
    saveRange(normalised);
    savePeriod(normalised.to);
    setRangeState(normalised);
  }, []);
  const setPeriod = useCallback((next: string) => setRange({ from: next, to: next }), [setRange]);
  const setCurrency = useCallback((next: string) => {
    saveCurrency(next);
    setCurrencyState(next);
  }, []);

  const session: Session = {
    client: apiKey ? { baseUrl, apiKey } : { baseUrl },
    person,
    period,
    setPeriod,
    range,
    setRange,
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
  const page = renderRoute(route);
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
            <PeriodPicker range={range} onChange={setRange} />
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

function renderRoute(route: string) {
  if (route === '' || route === 'overview') return <Overview />;
  if (route === 'journal') return <Journal />;
  if (route.startsWith('journal/')) return <EntryDetail entryId={decodeURIComponent(route.slice('journal/'.length))} />;
  if (route === 'accounts') return <Accounts />;
  if (route === 'accounts/new') return <AccountNew />;
  if (route.startsWith('accounts/')) return <AccountDetail number={decodeURIComponent(route.slice('accounts/'.length))} />;
  if (route === 'reports') return <Reports />;
  if (route === 'approvals') return <Approvals />;
  if (route === 'settings' || route.startsWith('settings/')) return <Settings section={settingsSectionOf(route)} />;
  if (route.startsWith('approvals/')) return <ApprovalDetail approvalId={decodeURIComponent(route.slice('approvals/'.length))} />;
  if (route === 'bills') return <Inbox />;
  if (route.startsWith('bills/')) return <BillReview billId={decodeURIComponent(route.slice('bills/'.length))} />;
  if (route === 'bank') return <BankAccounts />;
  if (route === 'bank/new') return <BankAccountNew />;
  if (route === 'bank/rules') return <BankRules />;
  if (route.startsWith('bank/')) return <BankAccountPage id={decodeURIComponent(route.slice('bank/'.length))} />;
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
