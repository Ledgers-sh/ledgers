/**
 * The logo picker for a company, a group or a business unit: a PNG or JPEG of
 * at most 2 MB, uploaded to the company's documents. The server checks the
 * same rule again on save. The field shows the logo as it will appear.
 */
import { useState } from 'preact/hooks';
import { uploadDocument } from '../api/bills';
import { ApiError } from '../api/client';
import { t, type MessageKey } from '../i18n';
import { logoProblem } from '../lib/logos';
import { useSession } from '../session';
import { Avatar, type LogoRef } from './Avatar';

export function LogoField({
  labelId,
  name,
  logo,
  onChange,
}: {
  labelId: string;
  /** The name the avatar falls back to (initials) when there is no logo. */
  name: string;
  /** The current logo, or null for none. */
  logo: LogoRef | null;
  /** Called with the stored document's id, or null when the logo is removed. */
  onChange: (documentId: string | null) => void;
}) {
  const client = useSession().client;
  const [uploading, setUploading] = useState(false);
  const [error, setError] = useState<MessageKey | null>(null);
  const [status, setStatus] = useState<MessageKey>(logo ? 'logo.set' : 'logo.none');

  const pick = async (file: File | undefined) => {
    if (!file) return;
    setError(null);
    const problem = logoProblem(file);
    if (problem) {
      setError(problem === 'too-large' ? 'logo.too_large' : 'logo.unsupported');
      return;
    }
    setUploading(true);
    try {
      const stored = await uploadDocument(client, file, file.name);
      onChange(stored.document.id);
      setStatus('logo.set');
    } catch (caught) {
      setError(caught instanceof ApiError && caught.status === 413 ? 'logo.too_large' : 'logo.failed');
    } finally {
      setUploading(false);
    }
  };

  return (
    <div class="field logo-field-block">
      <span id={labelId}>{t('logo.label')}</span>
      <div class="logo-field">
        <Avatar name={name} logo={logo} size="lg" />
        <input
          type="file"
          accept="image/png,image/jpeg"
          aria-labelledby={labelId}
          disabled={uploading}
          onChange={(event) => void pick((event.target as HTMLInputElement).files?.[0])}
        />
        <span class="muted" role="status">
          {uploading ? t('logo.uploading') : t(status)}
        </span>
        {logo && !uploading && (
          <button
            type="button"
            class="btn btn-ghost btn-sm"
            onClick={() => {
              onChange(null);
              setStatus('logo.none');
            }}
          >
            {t('logo.remove')}
          </button>
        )}
      </div>
      <span class="muted logo-hint">{t('logo.hint')}</span>
      {error && <span class="field-error">{t(error)}</span>}
    </div>
  );
}
