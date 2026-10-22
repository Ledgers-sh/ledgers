import { describe, expect, it } from 'vitest';
import { buildPdf, quickCheck, sha256Hex, type ScanPage } from './documents';

const page = (width: number, height: number): ScanPage => ({
  jpeg: new Uint8Array([0xff, 0xd8, 0xff, 0xd9]),
  width,
  height,
});

describe('documents', () => {
  it('hashes bytes to SHA-256 hex', async () => {
    expect(await sha256Hex(new TextEncoder().encode('abc'))).toBe(
      'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad',
    );
  });

  it('writes a PDF with one page per scanned image and a valid cross-reference table', () => {
    const pdf = buildPdf([page(1200, 1600), page(800, 600)]);
    const text = new TextDecoder('latin1').decode(pdf);
    expect(text.startsWith('%PDF-1.4')).toBe(true);
    expect(text).toContain('/Type /Pages /Kids [3 0 R 6 0 R] /Count 2');
    expect(text.match(/\/Subtype \/Image/g)?.length).toBe(2);
    expect(text).toContain('/Filter /DCTDecode');
    // Each xref entry points at its object: the offset of "N 0 obj" must match.
    const xrefAt = Number(/startxref\n(\d+)/.exec(text)?.[1]);
    expect(text.slice(xrefAt, xrefAt + 4)).toBe('xref');
    const entries = [...text.slice(xrefAt).matchAll(/^(\d{10}) 00000 n $/gm)].map((m) => Number(m[1]));
    entries.forEach((offset, index) => {
      expect(text.startsWith(`${index + 1} 0 obj`, offset)).toBe(true);
    });
  });

  it('refuses to write an empty document', () => {
    expect(() => buildPdf([])).toThrow();
  });

  it('checks size and type before queueing, leaving the verdict to the server', () => {
    expect(quickCheck({ size: 11 * 1024 * 1024, type: 'application/pdf', name: 'a.pdf' })).toBe('too-large');
    expect(quickCheck({ size: 10, type: 'text/plain', name: 'notes.txt' })).toBe('unsupported');
    expect(quickCheck({ size: 10, type: 'image/heic', name: 'IMG_1.HEIC' })).toBeNull();
    expect(quickCheck({ size: 10, type: '', name: 'scan.jpeg' })).toBeNull();
  });
});
