import { useState } from 'preact/hooks';
import { billsMatch } from '../api/commands';
import { useSession } from '../session';
import { ErrorBanner, PageHead } from './common';
import { t } from '../i18n';

/**
 * bills.match: POST /v1/bills/{id}/match. The command is not in the registry
 * yet, so the form sends only the bill id and shows the response as returned.
 */
export function Bills() {
  const { client, online, toast } = useSession();
  const [billId, setBillId] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const [result, setResult] = useState<Record<string, unknown> | null>(null);

  const submit = async (event: Event) => {
    event.preventDefault();
    const id = billId.trim();
    if (id === '') {
      setError(new Error(t('bills.enter_id')));
      return;
    }
    setBusy(true);
    setError(null);
    setResult(null);
    try {
      const response = await billsMatch(client, id);
      setResult(response);
      toast(t('bills.done'));
    } catch (err) {
      setError(err);
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <PageHead title={t('bills.title')}>{t('bills.intro')}</PageHead>
      <section class="panel" aria-labelledby="match-heading">
        <div class="panel-head">
          <h2 id="match-heading" class="grow">
            {t('bills.heading')}
          </h2>
        </div>
        <form class="panel-body entry-form" onSubmit={submit} noValidate>
          <label class="field">
            <span>{t('bills.id_label')}</span>
            <input class="input mono" value={billId} onInput={(e) => setBillId((e.target as HTMLInputElement).value)} autoComplete="off" />
            <span class="hint">{t('bills.id_hint')}</span>
          </label>
          {error !== null && <ErrorBanner error={error} />}
          <div class="form-actions">
            {!online && <span class="muted grow">{t('common.offline_posting')}</span>}
            <button type="submit" class="btn btn-primary" disabled={busy || !online} aria-busy={busy ? 'true' : undefined}>
              {busy ? t('bills.matching') : t('bills.run')}
            </button>
          </div>
        </form>
      </section>
      {result && (
        <section class="panel" aria-label={t('bills.result_section')}>
          <div class="panel-head">
            <h2 class="grow">{t('bills.result')}</h2>
          </div>
          <div class="panel-body">
            <pre class="result-json">{JSON.stringify(result, null, 2)}</pre>
          </div>
        </section>
      )}
    </>
  );
}
