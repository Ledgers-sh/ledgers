/**
 * Logos for companies, groups and business units, and the avatar that shows
 * them. A logo is a stored PNG or JPEG of at most 2 MiB (the server checks the
 * same rule on every write). Without one, the avatar shows initials on a colour.
 */

/** The largest logo the server stores: 2 MiB. */
export const MAX_LOGO_BYTES = 2 * 1024 * 1024;

/** The types a logo may have. SVG is never accepted: it can carry script. */
export const LOGO_TYPES = ['image/png', 'image/jpeg'] as const;

export type LogoProblem = 'unsupported' | 'too-large' | null;

/** Why a file cannot be a logo, or null when it can. The server still decides. */
export function logoProblem(file: { size: number; type: string }): LogoProblem {
  if (!(LOGO_TYPES as readonly string[]).includes(file.type)) return 'unsupported';
  if (file.size > MAX_LOGO_BYTES) return 'too-large';
  return null;
}

/** The initials of a name: the first letter of its first two words (one for a single word). */
export function initialsOf(name: string): string {
  const words = name
    .trim()
    .split(/[\s.,()&/-]+/u)
    .filter((word) => word.length > 0);
  if (words.length === 0) return '?';
  const first = Array.from(words[0] ?? '')[0] ?? '?';
  const second = words.length > 1 ? Array.from(words[1] ?? '')[0] : undefined;
  return (first + (second ?? '')).toLocaleUpperCase();
}

/** The fallback colour palette: the Ledgers unit colours, a deliberately small set. */
export const AVATAR_PALETTE = ['#7FA6C9', '#E9B44C', '#3D5A80', '#6FBF9A', '#C58AB8', '#D9825B'] as const;

/** A colour for a name that has none of its own: the same name always gets the same colour. */
export function fallbackColour(name: string): string {
  let hash = 0;
  for (const char of name) hash = (hash * 31 + (char.codePointAt(0) ?? 0)) >>> 0;
  return AVATAR_PALETTE[hash % AVATAR_PALETTE.length] ?? AVATAR_PALETTE[0];
}

/** Relative luminance of a `#RRGGBB` colour (WCAG), or null when it is not one. */
function luminanceOf(hex: string): number | null {
  const match = /^#([0-9a-f]{6})$/i.exec(hex);
  if (!match) return null;
  const value = parseInt(match[1] ?? '000000', 16);
  const channel = (shift: number) => {
    const c = ((value >> shift) & 0xff) / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0);
}

/**
 * The text colour that reads best on a `#RRGGBB` background: whichever of the
 * dark ink or white has the higher contrast ratio (WCAG), so an initial is
 * never white on a light colour or dark on a deep one.
 */
export function textOn(hex: string): string {
  const bg = luminanceOf(hex);
  if (bg === null) return '#0B0D10';
  const ink = luminanceOf('#0B0D10') ?? 0;
  const darkRatio = (bg + 0.05) / (ink + 0.05);
  const whiteRatio = 1.05 / (bg + 0.05);
  return darkRatio >= whiteRatio ? '#0B0D10' : '#FFFFFF';
}
