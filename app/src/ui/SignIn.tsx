import { signInUrl } from '../api/auth';
import { Icon } from './icons';
import { t } from '../i18n';
import { LanguagePicker } from '../i18n/LanguagePicker';

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
        <div class="brand connect-brand">
          <img src="/assets/favicon.svg" alt="" width="40" height="40" />
          <h1 style={{ font: 'inherit' }}>
            ledgers<span>.sh</span>
          </h1>
          <LanguagePicker />
        </div>
        <div>
          <h2 class="connect-title">{t('signin.title')}</h2>
          <p class="muted">{t('signin.body')}</p>
        </div>
        <a class="btn btn-primary btn-block" href={signInUrl(baseUrl)}>
          <Icon name="overview" />
          {t('signin.button')}
        </a>
        <details class="explainer">
          <summary>{t('signin.passkey_question')}</summary>
          <p class="muted">{t('signin.passkey_answer')}</p>
        </details>
        <div role="status" aria-live="polite">
          {error && (
            <p class="result fail" role="alert">
              <span>{error}</span>
            </p>
          )}
        </div>
        <p class="note">
          {t('signin.advanced_note')}{' '}
          <button type="button" class="link-button" onClick={onAdvanced}>
            {t('signin.advanced_button')}
          </button>
        </p>
      </div>
    </main>
  );
}
