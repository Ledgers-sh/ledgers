import { t } from '../i18n';
import { LanguagePicker } from '../i18n/LanguagePicker';
import { Icon } from './icons';

/**
 * The screen for a signed-in account that holds no membership in the company.
 * The title is the server's own problem title; the copy (ours, in the reader's
 * language) tells the person to ask the owner for an invitation.
 */
export function NotMember({ title, onSignOut }: { title: string; onSignOut: () => void }) {
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
        <div class="people-stack" role="alert">
          <h2 class="connect-title">{title}</h2>
          <p class="muted">{t('notMember.body')}</p>
        </div>
        <div class="row-actions">
          <button type="button" class="btn btn-primary" onClick={onSignOut}>
            <Icon name="logout" />
            {t('shell.sign_out')}
          </button>
        </div>
      </div>
    </main>
  );
}
