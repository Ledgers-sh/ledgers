import { signInUrl } from '../api/auth';
import { Icon } from './icons';

/**
 * The sign-in screen. A person signs in with a passkey: the button starts the
 * server's sign-in, which hands back to this app with a session cookie. There
 * is no password field.
 */
export function SignIn({
  baseUrl,
  error,
  onAdvanced,
}: {
  baseUrl: string;
  error: string | null;
  onAdvanced: () => void;
}) {
  return (
    <main class="connect" id="main">
      <div class="connect-card">
        <div class="brand">
          <img src="/assets/favicon.svg" alt="" width="40" height="40" />
          <h1 style={{ font: 'inherit' }}>
            ledgers<span>.sh</span>
          </h1>
        </div>
        <div>
          <h2 class="connect-title">Sign in to your books</h2>
          <p class="muted">Use the passkey you set up for this ledger. Nothing to type, and nothing a look-alike site can copy.</p>
        </div>
        <a class="btn btn-primary btn-block" href={signInUrl(baseUrl)}>
          <Icon name="overview" />
          Sign in with passkey
        </a>
        <details class="explainer">
          <summary>What is a passkey?</summary>
          <p class="muted">
            A passkey is a key kept by your device or password manager. It proves it is you without a password. The first
            sign-in, or a new device, uses a link sent to your email to confirm the passkey.
          </p>
        </details>
        <div role="status" aria-live="polite">
          {error && (
            <p class="result fail" role="alert">
              <span>{error}</span>
            </p>
          )}
        </div>
        <p class="note">
          Connecting a server or automation with an agent key?{' '}
          <button type="button" class="link-button" onClick={onAdvanced}>
            Advanced: connect with an agent key
          </button>
        </p>
      </div>
    </main>
  );
}
