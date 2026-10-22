/**
 * The keyboard and filtering rules of the Select combobox, kept free of the
 * DOM so they can be unit tested. The component applies them; it does not
 * decide them.
 */

export interface Choice {
  label: string;
  hint?: string;
  disabled?: boolean;
}

/** Lower-cased, diacritics-folded text, so "Oesterreich" and "Österreich" match. */
export function fold(text: string): string {
  return text.normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase();
}

/** The choices whose label or hint contains the query (folded). An empty query keeps them all, in order. */
export function filterChoices<T extends Choice>(choices: readonly T[], query: string): T[] {
  const needle = fold(query.trim());
  if (!needle) return [...choices];
  return choices.filter((choice) => fold(`${choice.label} ${choice.hint ?? ''}`).includes(needle));
}

/**
 * The index one step from `from` in `delta` direction (usually ±1), wrapping
 * and skipping disabled choices. Stays put when every choice is disabled or
 * there are none.
 */
export function stepIndex<T extends Choice>(choices: readonly T[], from: number, delta: number): number {
  if (choices.length === 0) return -1;
  let index = from;
  for (let step = 0; step < choices.length; step += 1) {
    index = (index + delta + choices.length) % choices.length;
    if (!choices[index].disabled) return index;
  }
  return from;
}

/** The first enabled choice at or after `index`, wrapping; -1 when none is enabled. */
export function firstEnabled<T extends Choice>(choices: readonly T[], index = 0): number {
  for (let step = 0; step < choices.length; step += 1) {
    const at = (index + step) % choices.length;
    if (!choices[at].disabled) return at;
  }
  return -1;
}

/**
 * Typeahead: the next enabled choice after `active` whose label starts with
 * `letter` (folded), searching round the list. `active` itself is last, so
 * repeating a letter cycles through the matches. Returns `active` when nothing
 * matches.
 */
export function typeaheadIndex<T extends Choice>(choices: readonly T[], active: number, letter: string): number {
  const needle = fold(letter);
  if (!needle || choices.length === 0) return active;
  for (let step = 1; step <= choices.length; step += 1) {
    const at = (active + step + choices.length) % choices.length;
    if (!choices[at].disabled && fold(choices[at].label).startsWith(needle)) return at;
  }
  return active;
}

/** Opens upwards when there is more room above the trigger than below and the list would not fit below. */
export function placementFor(room: { below: number; above: number }, height: number): 'up' | 'down' {
  return room.below < height && room.above > room.below ? 'up' : 'down';
}

/** Index of a step in a 2-D grid of `columns` columns, staying within `count` items. Returns `from` when the step leaves the grid. */
export function gridStep(from: number, count: number, columns: number, key: string, rtl: boolean): number {
  const horizontal = rtl ? -1 : 1;
  let target = from;
  if (key === 'ArrowRight') target = from + horizontal;
  else if (key === 'ArrowLeft') target = from - horizontal;
  else if (key === 'ArrowDown') target = from + columns;
  else if (key === 'ArrowUp') target = from - columns;
  else return from;
  return target < 0 || target >= count ? from : target;
}
