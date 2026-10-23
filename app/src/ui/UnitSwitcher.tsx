/**
 * The business-unit switcher in the top bar. It names the scope every screen
 * reads: the whole company (consolidated) or one unit, by its code and colour.
 * The choice is written to the URL (`?unit=LDG`), so a link carries it. Hidden
 * when the company has no active units.
 */
import type { Unit } from '../api/types';
import { t } from '../i18n';
import { colourOf } from '../lib/units';
import { Select, type SelectOption } from './Select';
import { Avatar } from './Avatar';

export function UnitSwitcher({
  units,
  value,
  onChange,
}: {
  units: readonly Unit[];
  value: string | null;
  onChange: (code: string | null) => void;
}) {
  if (units.length === 0) return null;
  const options: SelectOption[] = [
    { value: 'all', label: t('units.all'), trigger: t('units.all_short') },
    ...units.map((unit) => ({
      value: unit.code,
      label: unit.code,
      hint: unit.name,
      swatch: colourOf(unit),
      leading: (
        <Avatar
          name={unit.name}
          colour={colourOf(unit)}
          logo={unit.logo_document_id ? { documentId: unit.logo_document_id } : null}
          size="xs"
        />
      ),
    })),
  ];
  return (
    <div class="unit-switch">
      <span class="unit-switch-label" id="unit-switch-label">
        {t('units.switch_label')}
      </span>
      <Select
        id="unit-switch"
        labelledBy="unit-switch-label"
        class="compact"
        value={value ?? 'all'}
        options={options}
        searchable={false}
        onChange={(next) => onChange(next === 'all' ? null : next)}
      />
    </div>
  );
}
