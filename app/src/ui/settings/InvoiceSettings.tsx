import { statusGet, type StatusView } from '../../api/settings';
import { useAsync } from '../../hooks';
import { t, type MessageKey } from '../../i18n';
import { useSession } from '../../session';
import { ErrorBanner } from '../common';
import { LoadingLine, SettingsPanel } from './common';

const READER_NAME: Record<string, MessageKey> = {
  minimax: 'settings.reader_minimax',
  anthropic: 'settings.reader_anthropic',
  rules: 'settings.reader_rules',
};

const FEATURE_TEXT: Record<string, MessageKey> = {
  sign_in: 'settings.off_sign_in',
  document_links: 'settings.off_document_links',
};

/** The reader sentence: what reads invoice photos now, and why it is not the one the server was asked for. */
export function readerSentence(status: StatusView): string {
  const { provider, configured, reason } = status.extraction;
  const name = READER_NAME[provider];
  const reading = name ? t(name) : t('settings.reader_other', { provider });
  if (reason === 'missing_key' && configured) {
    return `${t('settings.reader_missing_key', { provider: configured })} ${reading}`;
  }
  if (reason === 'unknown_provider' && configured) {
    return `${t('settings.reader_unknown_provider', { provider: configured })} ${reading}`;
  }
  return reading;
}

/** Settings → Invoice reading: which reader reads photos, what is off and why, and the document limits. Read only: the keys live in the server's settings. */
export function InvoiceSettings() {
  const { client } = useSession();
  const state = useAsync(() => statusGet(client), [client.baseUrl, client.apiKey]);

  if (state.error && !state.data) return <ErrorBanner error={state.error} onRetry={state.reload} />;
  if (!state.data) return <LoadingLine />;
  const status = state.data;
  const megabytes = Math.round(status.limits.document_max_bytes / (1024 * 1024));

  return (
    <>
      <SettingsPanel id="reader-title" title={t('settings.reader_title')}>
        <p class="status-reader" role="status">
          {readerSentence(status)}
        </p>
        <p class="hint">{t('settings.reader_server_only')}</p>
      </SettingsPanel>

      <SettingsPanel id="off-title" title={t('settings.off_title')}>
        {status.unavailable.length === 0 ? (
          <p class="muted">{t('settings.off_none')}</p>
        ) : (
          <ul class="plain-list">
            {status.unavailable.map((item) => (
              <li key={item.feature}>
                <strong>{t(FEATURE_TEXT[item.feature] ?? 'settings.off_other', { feature: item.feature })}</strong>
                <span class="hint block">{item.reason}</span>
              </li>
            ))}
          </ul>
        )}
      </SettingsPanel>

      <SettingsPanel id="limits-title" title={t('settings.limits_title')}>
        <dl class="settings-facts">
          <dt>{t('settings.limit_size')}</dt>
          <dd>{t('settings.limit_size_value', { mb: megabytes })}</dd>
          <dt>{t('settings.limit_pages')}</dt>
          <dd>{t('settings.limit_pages_value', { pages: status.limits.pdf_max_pages })}</dd>
          <dt>{t('settings.limit_types')}</dt>
          <dd>{t('settings.limit_types_value')}</dd>
        </dl>
      </SettingsPanel>
    </>
  );
}
