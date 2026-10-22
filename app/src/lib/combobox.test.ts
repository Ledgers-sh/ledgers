import { describe, expect, it } from 'vitest';
import { filterChoices, firstEnabled, gridStep, placementFor, stepIndex, typeaheadIndex } from './combobox';

const choices = [
  { label: 'Bank', hint: '1020' },
  { label: 'Cash on hand', hint: '1010' },
  { label: 'Österreich' },
  { label: 'Accounts receivable', hint: '1030', disabled: true },
  { label: 'Office', hint: '6100' },
];

describe('filtering', () => {
  it('keeps everything, in order, for an empty query', () => {
    expect(filterChoices(choices, '  ')).toHaveLength(5);
  });

  it('matches label and hint, ignoring case and accents', () => {
    expect(filterChoices(choices, 'CASH').map((c) => c.label)).toEqual(['Cash on hand']);
    expect(filterChoices(choices, '1020').map((c) => c.label)).toEqual(['Bank']);
    expect(filterChoices(choices, 'osterreich').map((c) => c.label)).toEqual(['Österreich']);
    expect(filterChoices(choices, 'nothing here')).toEqual([]);
  });
});

describe('moving the active option', () => {
  it('wraps around and skips disabled options', () => {
    expect(stepIndex(choices, 4, 1)).toBe(0);
    expect(stepIndex(choices, 0, -1)).toBe(4);
    expect(stepIndex(choices, 1, 1)).toBe(2);
    expect(stepIndex(choices, 2, 1)).toBe(4);
  });

  it('stays put when every option is disabled, and is -1 for an empty list', () => {
    const disabled = [{ label: 'a', disabled: true }];
    expect(stepIndex(disabled, 0, 1)).toBe(0);
    expect(stepIndex([], 0, 1)).toBe(-1);
    expect(firstEnabled(choices, 3)).toBe(4);
    expect(firstEnabled(disabled)).toBe(-1);
  });
});

describe('typeahead', () => {
  it('jumps to the next option starting with the letter, cycling on repeats', () => {
    expect(typeaheadIndex(choices, 0, 'o')).toBe(2);
    expect(typeaheadIndex(choices, 2, 'o')).toBe(4);
    expect(typeaheadIndex(choices, 4, 'b')).toBe(0);
    expect(typeaheadIndex(choices, 1, 'a')).toBe(1);
  });

  it('does not land on a disabled option and keeps the active one when nothing matches', () => {
    expect(typeaheadIndex(choices, 0, 'a')).toBe(0);
    expect(typeaheadIndex(choices, 0, 'z')).toBe(0);
  });
});

describe('placement and grids', () => {
  it('opens upwards only when there is not room below and more above', () => {
    expect(placementFor({ below: 100, above: 600 }, 300)).toBe('up');
    expect(placementFor({ below: 500, above: 600 }, 300)).toBe('down');
    expect(placementFor({ below: 100, above: 50 }, 300)).toBe('down');
  });

  it('moves through a month grid, mirrored in right-to-left', () => {
    expect(gridStep(4, 12, 3, 'ArrowRight', false)).toBe(5);
    expect(gridStep(4, 12, 3, 'ArrowRight', true)).toBe(3);
    expect(gridStep(4, 12, 3, 'ArrowDown', false)).toBe(7);
    expect(gridStep(10, 12, 3, 'ArrowDown', false)).toBe(10);
    expect(gridStep(0, 12, 3, 'ArrowUp', false)).toBe(0);
    expect(gridStep(0, 12, 3, 'Enter', false)).toBe(0);
  });
});
