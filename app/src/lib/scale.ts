/**
 * Linear scales for the SVG charts. Geometry is plain numbers; money is turned
 * into a number only here, for drawing, never for totals.
 */

const STEPS = [1, 1.5, 2, 2.5, 3, 4, 5, 6, 8, 10];

/** The smallest "nice" value (a step times a power of ten) at or above `value`. */
export function niceCeil(value: number): number {
  if (!(value > 0)) return 1;
  const power = 10 ** Math.floor(Math.log10(value));
  const fraction = value / power;
  const nice = STEPS.find((step) => step >= fraction) ?? 10;
  return nice * power;
}

/**
 * Domain for values that may be negative: always includes zero, and both ends
 * fall on multiples of one nice step, so the `count` intervals carry round
 * tick labels (for example -2k, 0, 2k) rather than odd ones.
 */
export function domainOf(values: readonly number[], count = 4): { min: number; max: number } {
  const max = Math.max(0, ...values);
  const min = Math.min(0, ...values);
  if (max === 0 && min === 0) return { min: 0, max: 0 };
  const step = niceCeil((max - min) / count);
  return {
    min: Math.floor(min / step) * step,
    max: Math.ceil(max / step) * step,
  };
}

/** Maps a value in `domain` onto a pixel position between `from` (domain min) and `to` (domain max). */
export function linear(domain: { min: number; max: number }, from: number, to: number): (value: number) => number {
  const span = domain.max - domain.min || 1;
  return (value) => from + ((value - domain.min) / span) * (to - from);
}

/** Evenly spaced tick values across a domain, `count` intervals long. */
export function ticks(domain: { min: number; max: number }, count = 4): number[] {
  return Array.from({ length: count + 1 }, (_, index) => domain.min + ((domain.max - domain.min) * index) / count);
}
