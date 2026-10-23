/** Pieces the group screens share: the section tabs, company avatars and the reason an action is not offered. */
import type { ComponentChildren } from 'preact';
import { t, type MessageKey } from '../../i18n';
import { Ltr } from '../../i18n/Ltr';
import type { GroupKind } from '../../api/groups';

export type GroupSection = 'companies' | 'agreements' | 'reconciliation' | 'view';

const SECTION_HREF: Record<GroupSection, string> = {
  companies: '#/group',
  agreements: '#/group/agreements',
  reconciliation: '#/group/reconciliation',
  view: '#/group/view',
};

const SECTION_LABEL: Record<GroupSection, MessageKey> = {
  companies: 'groups.tab.companies',
  agreements: 'groups.tab.agreements',
  reconciliation: 'groups.tab.reconciliation',
  view: 'groups.tab.view',
};

export function GroupTabs({ current }: { current: GroupSection }) {
  return (
    <nav class="tabs group-tabs" aria-label={t('groups.tabs_label')}>
      {(Object.keys(SECTION_HREF) as GroupSection[]).map((section) => (
        <a key={section} class="tab" href={SECTION_HREF[section]} aria-current={current === section ? 'page' : undefined}>
          {t(SECTION_LABEL[section])}
        </a>
      ))}
    </nav>
  );
}

const KIND_KEY: Record<GroupKind, MessageKey> = {
  holding: 'groups.kind.holding',
  ip_holding: 'groups.kind.ip_holding',
  operating: 'groups.kind.operating',
};

/** The type chip of a member: holding, IP holding or operating. Text, not colour alone. */
export function KindChip({ kind }: { kind: GroupKind }) {
  return <span class={`chip group-kind kind-${kind}`}>{t(KIND_KEY[kind])}</span>;
}

/** A company's avatar: its initial in a round badge, decorative (the name is read beside it). */
export function CompanyAvatar({ name }: { name: string }) {
  const initial = name.trim().charAt(0).toUpperCase() || '?';
  return (
    <span class="avatar company-avatar" aria-hidden="true">
      <Ltr>{initial}</Ltr>
    </span>
  );
}

/** A company name with its avatar, for parties and member lists. */
export function CompanyName({ name, hidden }: { name: string | null; hidden?: boolean }) {
  if (hidden || name === null) {
    return <span class="company-name muted">{t('groups.hidden_company')}</span>;
  }
  return (
    <span class="company-name">
      <CompanyAvatar name={name} />
      <Ltr>{name}</Ltr>
    </span>
  );
}

/** A reason shown in place of an action the person may not take. Says why, in words. */
export function RoleNotice({ children }: { children: ComponentChildren }) {
  return (
    <p class="group-notice muted" role="note">
      {children}
    </p>
  );
}

export function SectionHead({ title, children }: { title: string; children?: ComponentChildren }) {
  return (
    <div class="group-section-head">
      <h2>{title}</h2>
      {children && <p class="muted">{children}</p>}
    </div>
  );
}
