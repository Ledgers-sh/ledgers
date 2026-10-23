/**
 * A business unit as a picker option: its logo (or initials on its colour)
 * before the code, the name as the hint. Used by every unit picker so a unit
 * looks the same wherever it is chosen.
 */
import type { Unit } from '../api/types';
import { colourOf } from '../lib/units';
import { Avatar } from './Avatar';
import type { SelectOption } from './Select';

export function unitSelectOption(unit: Unit): SelectOption {
  return {
    value: unit.code,
    label: unit.code,
    hint: unit.name,
    leading: (
      <Avatar
        name={unit.name}
        colour={colourOf(unit)}
        logo={unit.logo_document_id ? { documentId: unit.logo_document_id } : null}
        size="xs"
      />
    ),
  };
}
