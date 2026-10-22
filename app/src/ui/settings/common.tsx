import type { ComponentChildren } from 'preact';
import { describeError } from '../common';
import { t } from '../../i18n';

/** A settings panel: a heading, optional actions, and the body. */
export function SettingsPanel({
  id,
  title,
  actions,
  children,
}: {
  id: string;
  title: string;
  actions?: ComponentChildren;
  children: ComponentChildren;
}) {
  return (
    <section class="panel settings-panel" aria-labelledby={id}>
      <div class="panel-head">
        <h2 id={id}>{title}</h2>
        {actions ? <div class="row-actions">{actions}</div> : null}
      </div>
      <div class="panel-body">{children}</div>
    </section>
  );
}

/** A server refusal, shown where the form is (never a silent failure). */
export function FormProblem({ error }: { error: unknown }) {
  if (!error) return null;
  const problem = describeError(error);
  return (
    <div class="banner error" role="alert">
      <div class="grow">
        <strong>{problem.title}</strong>
        {problem.detail ? <div>{problem.detail}</div> : null}
      </div>
    </div>
  );
}

/** A field's message under its input, linked with aria-describedby by the caller. */
export function FieldProblem({ id, message }: { id: string; message: string | null }) {
  if (!message) return null;
  return (
    <span id={id} class="field-error">
      {message}
    </span>
  );
}

/** The status of a person's bookkeeping record, shown as a badge. */
export function StateBadge({ tone, children }: { tone: 'open' | 'closed' | 'soft' | 'neutral'; children: ComponentChildren }) {
  return <span class={`badge state-${tone}`}>{children}</span>;
}

/** Loading and failed states for a settings read, with the same wording everywhere. */
export function LoadingLine() {
  return (
    <p class="muted" role="status">
      {t('settings.loading')}
    </p>
  );
}
