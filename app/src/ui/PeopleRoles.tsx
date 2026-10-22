import { t } from '../i18n';
import { capabilityColumns, capabilityLabelKey, isRoleName, matrixRows, roleLabelKey, type RoleName } from '../lib/people';
import type { RoleView } from '../api/people';
import { SettingsPanel } from './settings/common';
import { Icon } from './icons';

/** The words for a role, in the reader's language; the server's own word for a role the app does not know. */
export function roleText(role: string): string {
  const key = roleLabelKey(role);
  return key ? t(key) : role;
}

/**
 * Each role has its own shape as well as its colour and its name: the shape
 * is aria-hidden and the words always say the role.
 */
const GLYPH: Record<RoleName, string> = {
  owner: '◆',
  admin: '▲',
  accountant: '■',
  approver: '●',
  viewer: '○',
  auditor: '◇',
};

/** A role as a badge: its name always, with a shape and a tone per role. */
export function RoleBadge({ role }: { role: string }) {
  const known = isRoleName(role);
  return (
    <span class={`badge role-badge role-${known ? role : 'other'}`}>
      <span class="role-glyph" aria-hidden="true">
        {known ? GLYPH[role] : '◦'}
      </span>
      {roleText(role)}
    </span>
  );
}

/** The capability's words, or the server's own name for one the app does not know yet. */
function capabilityText(capability: string): string {
  const key = capabilityLabelKey(capability);
  return key ? t(key) : capability;
}

/** The roles and what each may do. Read only: the matrix comes from GET /v1/auth/roles. */
export function RolesPanel({ roles }: { roles: RoleView[] }) {
  const rows = matrixRows(roles);
  const columns = capabilityColumns(roles);
  return (
    <SettingsPanel id="people-roles-title" title={t('people.roles_title')}>
      <p class="muted">{t('people.roles_intro')}</p>
      <p class="muted people-small">{t('people.roles_legend')}</p>
      <div class="table-wrap">
        <table class="ledger cards role-matrix">
          <caption class="sr-only">{t('people.roles_title')}</caption>
          <thead>
            <tr>
              <th scope="col">{t('people.roles_col_role')}</th>
              {columns.map((capability) => (
                <th key={capability} scope="col">
                  {capabilityText(capability)}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr key={row.role}>
                <th scope="row" class="role-matrix-role">
                  <RoleBadge role={row.role} />
                </th>
                {row.cells.map((cell) => (
                  <td key={cell.capability} data-label={capabilityText(cell.capability)} class="role-matrix-cell">
                    {cell.granted ? (
                      <span class="mark yes">
                        <Icon name="check" />
                        <span class="sr-only">{t('people.matrix_yes')}</span>
                      </span>
                    ) : (
                      <span class="mark no">
                        <span aria-hidden="true">–</span>
                        <span class="sr-only">{t('people.matrix_no')}</span>
                      </span>
                    )}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </SettingsPanel>
  );
}
