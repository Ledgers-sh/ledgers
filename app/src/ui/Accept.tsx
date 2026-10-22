import { useEffect, useRef, useState } from 'preact/hooks';
import { fetchSession, NotAMemberError, signInUrl, signOut } from '../api/auth';
import { ApiError } from '../api/client';
import { invitationAccept } from '../api/people';
import { acceptErrorKey, acceptPath, acceptTokenFrom } from '../lib/people';
import { t } from '../i18n';
import { LanguagePicker } from '../i18n/LanguagePicker';
import { DEFAULT_SERVER_URL } from '../session';
import { describeError } from './common';
import { Icon } from './icons';

/** How long the success message shows before the app opens. */
const OPEN_APP_MS = 1500;

type AcceptState =
  | { kind: 'checking' }
  | { kind: 'no-token' }
  | { kind: 'signing-in' }
  | { kind: 'accepting' }
  | { kind: 'done' }
  | { kind: 'failed'; error: unknown };

/**
 * The invitation link (/accept?token=...). A person who is not signed in is
 * sent through sign-in and comes back here; a signed-in person (member or not)
 * accepts the invitation, which the server checks against their verified address.
 */
export function AcceptInvitation() {
  const token = acceptTokenFrom(window.location.search);
  const [state, setState] = useState<AcceptState>(() => (token ? { kind: 'checking' } : { kind: 'no-token' }));
  const started = useRef(false);

  useEffect(() => {
    if (!token || started.current) return;
    started.current = true;
    let live = true;
    const config = { baseUrl: DEFAULT_SERVER_URL };

    const run = async () => {
      // A not-a-member answer still means signed in: the invitation is what makes them a member.
      let signedIn = true;
      try {
        signedIn = (await fetchSession(DEFAULT_SERVER_URL)) !== null;
      } catch (error) {
        if (!(error instanceof NotAMemberError)) {
          if (live) setState({ kind: 'failed', error });
          return;
        }
      }
      if (!live) return;
      if (!signedIn) {
        setState({ kind: 'signing-in' });
        window.location.assign(signInUrl(DEFAULT_SERVER_URL, acceptPath(token)));
        return;
      }
      setState({ kind: 'accepting' });
      try {
        await invitationAccept(config, token);
        if (!live) return;
        setState({ kind: 'done' });
        window.setTimeout(() => window.location.assign('/#/overview'), OPEN_APP_MS);
      } catch (error) {
        if (live) setState({ kind: 'failed', error });
      }
    };
    void run();
    return () => {
      live = false;
    };
  }, []);

  return (
    <main class="connect" id="main">
      <div class="connect-card">
        <div class="brand connect-brand">
          <img src="/assets/favicon.svg" alt="" width="40" height="40" />
          <h1 style={{ font: 'inherit' }}>
            ledgers<span>.sh</span>
          </h1>
          <LanguagePicker />
        </div>
        <h2 class="connect-title">{t('accept.title')}</h2>
        <AcceptBody state={state} token={token} />
      </div>
    </main>
  );
}

function AcceptBody({ state, token }: { state: AcceptState; token: string | null }) {
  switch (state.kind) {
    case 'checking':
      return (
        <p class="muted" role="status">
          {t('accept.checking')}
        </p>
      );
    case 'no-token':
      return (
        <p class="result fail" role="alert">
          <span>{t('accept.no_token')}</span>
        </p>
      );
    case 'signing-in':
      return (
        <div class="people-stack" role="status">
          <p class="muted">{t('accept.signing_in')}</p>
          {token && (
            <p>
              <a class="btn btn-primary" href={signInUrl(DEFAULT_SERVER_URL, acceptPath(token))}>
                <Icon name="overview" />
                {t('accept.sign_in')}
              </a>
            </p>
          )}
        </div>
      );
    case 'accepting':
      return (
        <p class="muted" role="status">
          {t('accept.accepting')}
        </p>
      );
    case 'done':
      return (
        <div class="people-stack" role="status">
          <p class="result ok">
            <span>{t('accept.done_title')}</span>
          </p>
          <p class="muted">{t('accept.done_body')}</p>
          <p>
            <a class="btn btn-primary" href="/#/overview">
              {t('accept.open_app')}
            </a>
          </p>
        </div>
      );
    case 'failed':
      return <AcceptFailure error={state.error} token={token} />;
  }
}

/** A refused accept, in words the person can act on. The sign-out path covers the wrong-address case. */
function AcceptFailure({ error, token }: { error: unknown; token: string | null }) {
  const key = error instanceof ApiError ? acceptErrorKey(error) : null;
  const generic = describeError(error);
  return (
    <div class="people-stack">
      <div class="banner error" role="alert">
        <div class="grow">
          <strong>{key ? t(key) : t('accept.err_title')}</strong>
          {!key && generic.detail && <div class="muted">{generic.detail}</div>}
        </div>
      </div>
      <div class="row-actions">
        {key === 'accept.err_wrong_address' && (
          <button
            type="button"
            class="btn btn-primary"
            onClick={() => {
              // Sign out, then this page starts again: signed out, it sends the person to sign in for this invitation.
              void signOut(DEFAULT_SERVER_URL).then(() => window.location.reload());
            }}
          >
            <Icon name="logout" />
            {t('accept.sign_out')}
          </button>
        )}
        {key === 'accept.err_signed_out' && token && (
          <a class="btn btn-primary" href={signInUrl(DEFAULT_SERVER_URL, acceptPath(token))}>
            {t('accept.sign_in')}
          </a>
        )}
        {!key && (
          <button type="button" class="btn" onClick={() => window.location.reload()}>
            {t('common.try_again')}
          </button>
        )}
      </div>
    </div>
  );
}
