/**
 * Client-side document handling for uploads (issue #34): hashing for the
 * offline queue, HEIC and other photos converted to JPEG, and several camera
 * pages combined into one PDF. The server still sniffs every file's type from
 * its bytes; nothing here decides what the server accepts.
 */

/** The largest upload the server stores: 10 MiB. */
export const MAX_UPLOAD_BYTES = 10 * 1024 * 1024;
/** The most camera pages combined into one document. */
export const MAX_SCAN_PAGES = 20;

const encoder = new TextEncoder();

/** The SHA-256 of `bytes` as lowercase hex. The queue key and the dedupe key. */
export async function sha256Hex(bytes: ArrayBuffer | Uint8Array): Promise<string> {
  const buffer = bytes instanceof Uint8Array ? bytes.slice().buffer : bytes;
  const digest = await crypto.subtle.digest('SHA-256', buffer);
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, '0')).join('');
}

/** A JPEG page ready for the PDF writer. */
export interface ScanPage {
  jpeg: Uint8Array;
  width: number;
  height: number;
}

/** Decodes any image the browser can (PNG, JPEG, HEIC on Safari) and re-encodes it as JPEG. */
export async function imageToJpeg(blob: Blob): Promise<ScanPage> {
  const bitmap = await createImageBitmap(blob);
  try {
    const canvas = document.createElement('canvas');
    canvas.width = bitmap.width;
    canvas.height = bitmap.height;
    const context = canvas.getContext('2d');
    if (!context) throw new Error('no 2d canvas');
    context.fillStyle = '#ffffff';
    context.fillRect(0, 0, canvas.width, canvas.height);
    context.drawImage(bitmap, 0, 0);
    const jpeg = await new Promise<Blob>((resolve, reject) =>
      canvas.toBlob((out) => (out ? resolve(out) : reject(new Error('jpeg encode failed'))), 'image/jpeg', 0.9),
    );
    return { jpeg: new Uint8Array(await jpeg.arrayBuffer()), width: bitmap.width, height: bitmap.height };
  } finally {
    bitmap.close();
  }
}

/** Whether a file must be converted before upload (HEIC/HEIF photos). */
export function needsConversion(file: { type: string; name: string }): boolean {
  return /hei[cf]/.test(file.type) || /\.hei[cf]$/i.test(file.name);
}

/**
 * Writes one PDF whose pages are the given JPEGs, each scaled to fit an A4
 * page with a margin. No text layer: the model reads the scan.
 */
export function buildPdf(pages: ScanPage[]): Uint8Array {
  if (pages.length === 0) throw new Error('a PDF needs at least one page');
  const pageWidth = 595;
  const pageHeight = 842;
  const margin = 24;
  // Object numbers: 1 catalog, 2 page tree, then three per page: page, content, image.
  const objectCount = 2 + pages.length * 3;
  const chunks: Uint8Array[] = [];
  let length = 0;
  const offsets: number[] = [];
  const write = (part: string | Uint8Array) => {
    const bytes = typeof part === 'string' ? encoder.encode(part) : part;
    chunks.push(bytes);
    length += bytes.length;
  };
  const object = (number: number, body: string | Uint8Array) => {
    offsets[number] = length;
    write(`${number} 0 obj\n`);
    write(body);
    write('\nendobj\n');
  };
  write('%PDF-1.4\n%âãÏÓ\n');
  object(1, '<< /Type /Catalog /Pages 2 0 R >>');
  const kids = pages.map((_, index) => `${3 + index * 3} 0 R`).join(' ');
  object(2, `<< /Type /Pages /Kids [${kids}] /Count ${pages.length} >>`);
  pages.forEach((page, index) => {
    const pageNumber = 3 + index * 3;
    const contentNumber = pageNumber + 1;
    const imageNumber = pageNumber + 2;
    const scale = Math.min((pageWidth - 2 * margin) / page.width, (pageHeight - 2 * margin) / page.height);
    const drawWidth = Math.round(page.width * scale);
    const drawHeight = Math.round(page.height * scale);
    const x = Math.round((pageWidth - drawWidth) / 2);
    const y = Math.round((pageHeight - drawHeight) / 2);
    object(
      pageNumber,
      `<< /Type /Page /Parent 2 0 R /MediaBox [0 0 ${pageWidth} ${pageHeight}] /Contents ${contentNumber} 0 R ` +
        `/Resources << /XObject << /Im${index} ${imageNumber} 0 R >> >> >>`,
    );
    const content = `q ${drawWidth} 0 0 ${drawHeight} ${x} ${y} cm /Im${index} Do Q\n`;
    object(contentNumber, `<< /Length ${content.length} >>\nstream\n${content}endstream`);
    const header =
      `<< /Type /XObject /Subtype /Image /Width ${page.width} /Height ${page.height} /ColorSpace /DeviceRGB ` +
      `/BitsPerComponent 8 /Filter /DCTDecode /Length ${page.jpeg.length} >>\nstream\n`;
    offsets[imageNumber] = length;
    write(`${imageNumber} 0 obj\n${header}`);
    write(page.jpeg);
    write('\nendstream\nendobj\n');
  });
  const xrefAt = length;
  let xref = `xref\n0 ${objectCount + 1}\n0000000000 65535 f \n`;
  for (let number = 1; number <= objectCount; number += 1) {
    xref += `${String(offsets[number]).padStart(10, '0')} 00000 n \n`;
  }
  write(xref);
  write(`trailer\n<< /Size ${objectCount + 1} /Root 1 0 R >>\nstartxref\n${xrefAt}\n%%EOF\n`);
  const out = new Uint8Array(length);
  let at = 0;
  for (const chunk of chunks) {
    out.set(chunk, at);
    at += chunk.length;
  }
  return out;
}

/** The size and magic check the app shows before queueing; the server decides for real. */
export function quickCheck(file: { size: number; type: string; name: string }): 'too-large' | 'unsupported' | null {
  if (file.size > MAX_UPLOAD_BYTES) return 'too-large';
  const named = /\.(pdf|png|jpe?g|hei[cf])$/i.test(file.name);
  const typed = /^(application\/pdf|image\/(png|jpe?g|hei[cf]))$/.test(file.type);
  return named || typed ? null : 'unsupported';
}
