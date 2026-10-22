import { useState } from 'preact/hooks';
import { reportsTrialBalance } from '../api/commands';
import { ApiError, type ClientConfig } from '../api/client';
import { DEFAULT_SERVER_URL, type Credentials } from '../session';
import { currentPeriod } from '../lib/period';
import { Icon } from './icons';

type TestState = { status: 'idle' } | { status: 'busy' } | { status: 'ok'; message: string } | { status: 'fail'; message: string };

export function Connect({ onConnected }: { onConnected: (credentials: Credentials) => void }) {
  const [baseUrl, setBaseUrl] = useState(DEFAULT_SERVER_URL);
  const [apiKey, setApiKey] = useState('');
  const [test, setTest] = useState<TestState>({ status: 'idle' });
  const [urlError, setUrlError] = useState<string | null>(null);

  const validUrl = (): string | null => {
    try {
      const url = new URL(baseUrl.trim());
      if (url.protocol !== 'http:' && url.protocol !== 'https:') throw new Error('protocol');
      setUrlError(null);
      return url.origin;
    } catch {
      setUrlError('Enter a full URL, such as http://127.0.0.1:3000.');
      return null;
    }
  };

  const runTest = async () => {
    const origin = validUrl();
    if (!origin) return;
    if (apiKey.trim() === '') {
      setTest({ status: 'fail', message: 'Paste an API key first.' });
      return;
    }
    setTest({ status: 'busy' });
    const config: ClientConfig = { baseUrl: origin, apiKey: apiKey.trim() };
    try {
      const report = await reportsTrialBalance(config, currentPeriod());
      setTest({
        status: 'ok',
        message: `Connected. ${report.rows.length} account${report.rows.length === 1 ? '' : 's'} with activity in ${report.period}.`,
      });
    } catch (error) {
      const message = error instanceof ApiError ? error.message : 'Connection failed.';
      setTest({ status: 'fail', message: error instanceof ApiError && error.detail ? `${message} ${error.detail}` : message });
    }
  };

  const connect = (event: Event) => {
    event.preventDefault();
    const origin = validUrl();
    if (!origin || apiKey.trim() === '') {
      if (apiKey.trim() === '') setTest({ status: 'fail', message: 'Paste an API key first.' });
      return;
    }
    onConnected({ baseUrl: origin, apiKey: apiKey.trim() });
  };

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
          <p class="muted">Connect to a Ledgers server to review the books and post entries.</p>
        </div>
        <form onSubmit={connect} noValidate>
          <label class="field">
            <span>Server URL</span>
            <input
              class="input mono"
              type="url"
              name="baseUrl"
              inputMode="url"
              autoComplete="url"
              value={baseUrl}
              aria-invalid={urlError ? 'true' : undefined}
              aria-describedby={urlError ? 'url-error' : 'url-hint'}
              onInput={(event) => setBaseUrl((event.target as HTMLInputElement).value)}
              onBlur={validUrl}
            />
            {urlError ? (
              <span id="url-error" class="field-error">{urlError}</span>
            ) : (
              <span id="url-hint" class="hint">Local development: http://127.0.0.1:3000</span>
            )}
          </label>
          <label class="field">
            <span>API key</span>
            <input
              class="input mono"
              type="password"
              name="apiKey"
              autoComplete="off"
              spellcheck={false}
              value={apiKey}
              aria-describedby="key-hint"
              onInput={(event) => {
                setApiKey((event.target as HTMLInputElement).value);
                setTest({ status: 'idle' });
              }}
            />
            <span id="key-hint" class="hint">
              Kept in this tab's session storage only. Closing the tab forgets it.
            </span>
          </label>
          <div class="row-actions">
            <button type="button" class="btn" onClick={runTest} disabled={test.status === 'busy'} aria-busy={test.status === 'busy'}>
              {test.status === 'busy' ? 'Testing…' : 'Test connection'}
            </button>
            <button type="submit" class="btn btn-primary">
              Connect
            </button>
          </div>
          <div role="status" aria-live="polite">
            {test.status === 'ok' && (
              <p class="result ok">
                <Icon name="overview" />
                <span>{test.message}</span>
              </p>
            )}
            {test.status === 'fail' && (
              <p class="result fail" role="alert">
                <span>{test.message}</span>
              </p>
            )}
          </div>
        </form>
        <p class="note">
          Passkey sign-in arrives with issue #12. Until then this app uses an API key, and no passkey flow is shown.
        </p>
      </div>
    </main>
  );
}
