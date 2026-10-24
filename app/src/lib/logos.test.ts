import { describe, expect, it } from 'vitest';
import { AVATAR_PALETTE, MAX_LOGO_BYTES, fallbackColour, initialsOf, logoProblem, textOn } from './logos';

describe('logoProblem', () => {
  it('accepts a PNG or a JPEG within the limit', () => {
    expect(logoProblem({ size: 1024, type: 'image/png' })).toBeNull();
    expect(logoProblem({ size: MAX_LOGO_BYTES, type: 'image/jpeg' })).toBeNull();
  });

  it('refuses every other type, SVG included', () => {
    for (const type of ['image/svg+xml', 'application/pdf', 'image/heic', 'image/gif', '']) {
      expect(logoProblem({ size: 10, type })).toBe('unsupported');
    }
  });

  it('refuses a file one byte over the limit', () => {
    expect(logoProblem({ size: MAX_LOGO_BYTES + 1, type: 'image/png' })).toBe('too-large');
  });

  it('reports the type before the size', () => {
    expect(logoProblem({ size: MAX_LOGO_BYTES + 1, type: 'application/pdf' })).toBe('unsupported');
  });
});

describe('initialsOf', () => {
  it('takes the first letters of the first two words', () => {
    expect(initialsOf('Factory Zero IP Pte. Ltd.')).toBe('FZ');
    expect(initialsOf('Ledgers')).toBe('L');
    expect(initialsOf('PT Operasional Nusantara')).toBe('PO');
  });

  it('falls back to a question mark for an empty name', () => {
    expect(initialsOf('   ')).toBe('?');
  });

  it('reads non-Latin letters as whole characters', () => {
    expect(initialsOf('شركة الأمل')).toBe('شا');
  });
});

describe('fallbackColour', () => {
  it('is stable for a name and always a palette colour', () => {
    expect(fallbackColour('Sqeezin')).toBe(fallbackColour('Sqeezin'));
    expect(AVATAR_PALETTE).toContain(fallbackColour('Living Brain'));
    expect(AVATAR_PALETTE).toContain(fallbackColour(''));
  });
});

describe('textOn', () => {
  it('is dark on light colours and white on dark ones', () => {
    expect(textOn('#E9B44C')).toBe('#0B0D10');
    expect(textOn('#3D5A80')).toBe('#FFFFFF');
    expect(textOn('not a colour')).toBe('#0B0D10');
  });
});

describe('textOn contrast', () => {
  /** WCAG contrast ratio of two #RRGGBB colours. */
  function ratio(a: string, b: string): number {
    const lum = (hex: string) => {
      const v = parseInt(hex.slice(1), 16);
      const c = (shift: number) => {
        const x = ((v >> shift) & 0xff) / 255;
        return x <= 0.03928 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4;
      };
      return 0.2126 * c(16) + 0.7152 * c(8) + 0.0722 * c(0);
    };
    const [hi, lo] = [lum(a), lum(b)].sort((x, y) => y - x) as [number, number];
    return (hi + 0.05) / (lo + 0.05);
  }

  it('gives every fallback colour at least 4.5 to 1 with its initials', () => {
    for (const colour of AVATAR_PALETTE) {
      expect(ratio(colour, textOn(colour)), colour).toBeGreaterThanOrEqual(4.5);
    }
  });
});
