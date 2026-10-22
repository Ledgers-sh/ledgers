import { describe, expect, it } from 'vitest';
import { domainOf, linear, niceCeil, ticks } from './scale';

describe('scales', () => {
  it('rounds up to a nice bound', () => {
    expect(niceCeil(0.7)).toBe(0.8);
    expect(niceCeil(1.2)).toBe(1.5);
    expect(niceCeil(2.3)).toBe(2.5);
    expect(niceCeil(4_100)).toBe(5_000);
    expect(niceCeil(30_000)).toBe(30_000);
    expect(niceCeil(4_900)).toBe(5_000);
    expect(niceCeil(0)).toBe(1);
  });

  it('always includes zero and both signs', () => {
    expect(domainOf([120, 80, 30])).toEqual({ min: 0, max: 120 });
    expect(domainOf([-600, 2_400])).toEqual({ min: -800, max: 2_400 });
    expect(domainOf([-40, 90])).toEqual({ min: -40, max: 120 });
    expect(domainOf([0, 0])).toEqual({ min: 0, max: 0 });
    expect(domainOf([])).toEqual({ min: 0, max: 0 });
  });

  it('maps values linearly onto pixels', () => {
    const x = linear({ min: 0, max: 100 }, 0, 200);
    expect(x(0)).toBe(0);
    expect(x(25)).toBe(50);
    expect(linear({ min: -50, max: 50 }, 100, 0)(-50)).toBe(100);
  });

  it('gives evenly spaced ticks', () => {
    expect(ticks({ min: 0, max: 100 }, 4)).toEqual([0, 25, 50, 75, 100]);
  });
});
