/**
 * The Group screen: the group's name, its member companies with their type
 * (holding, IP holding or operating) and the ownership the group holds in
 * each. Owners create and change a group; the screen says why it cannot when
 * the person is not an owner of every member.
 */
import { useState } from 'preact/hooks';
import { groupsCreate, groupsUpdate, type GroupKind, type GroupView, type MemberInput } from '../../api/groups';
import { groupMembershipBlocker, formatBpPercent, parsePercentToBp, isOwner } from '../../lib/groups';
import { intlLocale, t, type MessageKey } from '../../i18n';
import { ErrorBanner, Empty, PageHead, Skeleton } from '../common';
import { Select, type SelectOption } from '../Select';
import { useGroupContext, type GroupContext } from './context';
import { CompanyName, GroupTabs, KindChip, RoleNotice } from './shared';

const KINDS: GroupKind[] = ['holding', 'ip_holding', 'operating'];
const KIND_LABEL: Record<GroupKind, MessageKey> = {
  holding: 'groups.kind.holding',
  ip_holding: 'groups.kind.ip_holding',
  operating: 'groups.kind.operating',
};

export function GroupPage() {
  const ctx = useGroupContext();
  const [groupId, setGroupId] = useState<string | null>(null);
  const [mode, setMode] = useState<'view' | 'edit' | 'create'>('view');
  const [notice, setNotice] = useState<string | null>(null);

  if (ctx.error) {
    return (
      <>
        <PageHead title={t('groups.title')}>{t('groups.intro')}</PageHead>
        <GroupTabs current="companies" />
        <ErrorBanner error={ctx.error} onRetry={ctx.reload} />
      </>
    );
  }
  if (!ctx.groups || !ctx.companies) {
    return (
      <>
        <PageHead title={t('groups.title')} />
        <Skeleton rows={3} label={t('groups.loading')} />
      </>
    );
  }
  const groups = ctx.groups;
  const group = groups.find((item) => item.id === groupId) ?? groups[0];
  const ownedCount = ctx.companies.filter((company) => isOwner(company.role)).length;
  const canCreate = ownedCount >= 2;

  const done = (saved: GroupView) => {
    setGroupId(saved.id);
    setMode('view');
    setNotice(t('groups.saved'));
    ctx.reload();
  };

  return (
    <>
      <PageHead title={t('groups.title')}>{t('groups.intro')}</PageHead>
      <GroupTabs current="companies" />
      {notice && (
        <div class="banner" role="status">
          <span class="grow">{notice}</span>
        </div>
      )}
      {groups.length === 0 && mode !== 'create' && (
        <Empty
          title={t('groups.empty_title')}
          action={
            canCreate ? (
              <button type="button" class="btn btn-primary" onClick={() => setMode('create')}>
                {t('groups.create')}
              </button>
            ) : undefined
          }
        >
          <p>{canCreate ? t('groups.empty_body') : t('groups.empty_needs_owner')}</p>
        </Empty>
      )}
      {mode === 'create' && (
        <GroupEditor
          ctx={ctx}
          group={null}
          onCancel={() => setMode('view')}
          onSaved={done}
        />
      )}
      {group && mode !== 'create' && (
        <section class="panel group-panel" aria-labelledby="group-name-heading">
          <div class="panel-head">
            {groups.length > 1 ? (
              <div class="field grow">
                <span id="group-pick-label">{t('groups.pick_group')}</span>
                <Select
                  id="group-pick"
                  labelledBy="group-pick-label"
                  value={group.id}
                  options={groups.map((item) => ({ value: item.id, label: item.name }))}
                  onChange={(value) => {
                    setGroupId(value);
                    setMode('view');
                  }}
                />
              </div>
            ) : (
              <h2 id="group-name-heading" class="grow">
                {group.name}
              </h2>
            )}
            {mode === 'view' && (
              <GroupActions ctx={ctx} group={group} onEdit={() => setMode('edit')} />
            )}
          </div>
          {mode === 'edit' ? (
            <div class="panel-body">
              <GroupEditor ctx={ctx} group={group} onCancel={() => setMode('view')} onSaved={done} />
            </div>
          ) : (
            <div class="panel-body">
              <div class="table-wrap">
                <table class="group-members">
                  <caption class="sr-only">{t('groups.members')}</caption>
                  <thead>
                    <tr>
                      <th scope="col">{t('groups.col.company')}</th>
                      <th scope="col">{t('groups.col.kind')}</th>
                      <th scope="col" class="num">
                        {t('groups.col.ownership')}
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {group.members.map((member) => (
                      <tr key={member.company_id}>
                        <td>
                          <CompanyName name={member.company_name} />
                        </td>
                        <td>
                          <KindChip kind={member.kind} />
                        </td>
                        <td class="num">
                          {formatBpPercent(member.ownership_bp, intlLocale())}%
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              {group.has_hidden_members && <RoleNotice>{t('groups.hidden_members')}</RoleNotice>}
            </div>
          )}
        </section>
      )}
    </>
  );
}

/** The edit button, or the reason it is not offered: the person must be an owner of every member. */
function GroupActions({ ctx, group, onEdit }: { ctx: GroupContext; group: GroupView; onEdit: () => void }) {
  const ids = group.members.map((member) => member.company_id);
  const blocker = groupMembershipBlocker(ids, ctx.companies ?? []);
  const ownerOf = ids.filter((id) => ctx.companies?.some((company) => company.id === id && isOwner(company.role)));
  const blockedBy = ids.find((id) => !ownerOf.includes(id));
  return (
    <div class="row-actions">
      <button
        type="button"
        class="btn btn-sm"
        disabled={blocker !== null}
        aria-describedby={blocker !== null ? 'group-edit-reason' : undefined}
        onClick={onEdit}
      >
        {t('groups.edit')}
      </button>
      {blocker !== null && (
        <RoleNotice>
          <span id="group-edit-reason">
            {blocker === 'not_owner' && blockedBy
              ? t('groups.not_owner', { company: ctx.nameOf(blockedBy) })
              : t('groups.needs_two')}
          </span>
        </RoleNotice>
      )}
    </div>
  );
}

interface MemberRow {
  key: number;
  companyId: string;
  kind: GroupKind;
  percent: string;
}

/** Creates a group (group null) or changes a group's name and members. */
function GroupEditor({
  ctx,
  group,
  onCancel,
  onSaved,
}: {
  ctx: GroupContext;
  group: GroupView | null;
  onCancel: () => void;
  onSaved: (saved: GroupView) => void;
}) {
  const [name, setName] = useState(group?.name ?? '');
  const [nextKey, setNextKey] = useState(100);
  const [rows, setRows] = useState<MemberRow[]>(() =>
    group
      ? group.members.map((member, index) => ({
          key: index,
          companyId: member.company_id,
          kind: member.kind,
          percent: formatBpPercent(member.ownership_bp, intlLocale()),
        }))
      : [
          { key: 0, companyId: '', kind: 'holding', percent: '100' },
          { key: 1, companyId: '', kind: 'operating', percent: '100' },
        ],
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const companies = ctx.companies ?? [];
  const companyOptions: SelectOption[] = companies.map((company) => ({
    value: company.id,
    label: company.name,
    disabled: !isOwner(company.role),
    hint: isOwner(company.role) ? undefined : t('groups.owner_only_hint'),
  }));

  const update = (key: number, patch: Partial<MemberRow>) =>
    setRows((current) => current.map((row) => (row.key === key ? { ...row, ...patch } : row)));

  const save = async () => {
    setProblem(null);
    const ids = rows.map((row) => row.companyId).filter((id) => id !== '');
    if (name.trim() === '') return setProblem(t('groups.name_required'));
    const blocker = groupMembershipBlocker(ids, companies);
    if (blocker === 'fewer_than_two' || ids.length !== rows.length) return setProblem(t('groups.needs_two'));
    if (blocker === 'not_owner') {
      const blocked = ids.find((id) => !isOwner(companies.find((company) => company.id === id)?.role));
      return setProblem(t('groups.not_owner', { company: ctx.nameOf(blocked ?? '') }));
    }
    if (blocker === 'unknown_company') return setProblem(t('groups.needs_two'));
    if (new Set(ids).size !== ids.length) return setProblem(t('groups.duplicate_company'));
    const members: MemberInput[] = [];
    for (const row of rows) {
      const bp = parsePercentToBp(row.percent);
      if (bp === null) return setProblem(t('groups.percent_invalid'));
      members.push({ company_id: row.companyId, kind: row.kind, ownership_bp: bp });
    }
    setBusy(true);
    try {
      const saved = group
        ? await groupsUpdate(ctx.client, group.id, { name: name.trim(), members })
        : await groupsCreate(ctx.client, { name: name.trim(), members });
      onSaved(saved);
    } catch (caught) {
      setError(caught);
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      class="group-editor"
      aria-labelledby="group-editor-title"
      onSubmit={(event) => {
        event.preventDefault();
        void save();
      }}
    >
      <h2 id="group-editor-title">{group ? t('groups.edit_title') : t('groups.new_title')}</h2>
      <div class="field">
        <span id="group-name-label">{t('groups.name')}</span>
        <input
          aria-labelledby="group-name-label"
          id="group-name"
          class="input"
          value={name}
          onInput={(event) => setName((event.target as HTMLInputElement).value)}
        />
      </div>
      <fieldset class="field-group">
        <legend>{t('groups.members')}</legend>
        {rows.map((row, index) => (
          <div class="group-member-row" key={row.key}>
            <div class="field">
              <span id={`gm-company-label-${row.key}`}>{t('groups.col.company')}</span>
              <Select
                id={`gm-company-${row.key}`}
                labelledBy={`gm-company-label-${row.key}`}
                value={row.companyId}
                options={companyOptions}
                placeholder={t('groups.choose_company')}
                emptyLabel={t('groups.choose_company')}
                onChange={(value) => update(row.key, { companyId: value })}
              />
            </div>
            <div class="field">
              <span id={`gm-kind-label-${row.key}`}>{t('groups.col.kind')}</span>
              <Select
                id={`gm-kind-${row.key}`}
                labelledBy={`gm-kind-label-${row.key}`}
                value={row.kind}
                options={KINDS.map((kind) => ({ value: kind, label: t(KIND_LABEL[kind]) }))}
                onChange={(value) => update(row.key, { kind: value as GroupKind })}
              />
            </div>
            <div class="field">
              <span id={`gm-percent-label-${row.key}`}>{t('groups.col.ownership')}</span>
              <input
                aria-labelledby={`gm-percent-label-${row.key}`}
                class="input num"
                inputMode="decimal"
                value={row.percent}
                onInput={(event) => update(row.key, { percent: (event.target as HTMLInputElement).value })}
              />
            </div>
            <button
              type="button"
              class="btn btn-ghost btn-sm"
              disabled={rows.length <= 2}
              onClick={() => setRows((current) => current.filter((item) => item.key !== row.key))}
              aria-label={t('groups.remove_row', { index: index + 1 })}
            >
              {t('groups.remove')}
            </button>
          </div>
        ))}
        <div class="row-actions">
          <button
            type="button"
            class="btn btn-sm"
            onClick={() => {
              setRows((current) => [...current, { key: nextKey, companyId: '', kind: 'operating', percent: '100' }]);
              setNextKey((value) => value + 1);
            }}
          >
            {t('groups.add_member')}
          </button>
        </div>
      </fieldset>
      {problem && (
        <p class="field-error" role="alert">
          {problem}
        </p>
      )}
      {error !== null && <ErrorBanner error={error} />}
      <div class="row-actions">
        <button type="submit" class="btn btn-primary" disabled={busy}>
          {group ? t('groups.save') : t('groups.create')}
        </button>
        <button type="button" class="btn" onClick={onCancel}>
          {t('groups.cancel')}
        </button>
      </div>
    </form>
  );
}
