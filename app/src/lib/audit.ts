/**
 * The audit log's CSV export: the filtered events, one row each, in the order
 * the page shows them. Cells are quoted when they hold a comma, quote or line
 * break, and a quote inside a cell is doubled (RFC 4180).
 */
import type { AuditEventView } from '../api/settings';

export const AUDIT_CSV_HEADER = ['seq', 'at', 'actor_kind', 'actor_id', 'command', 'outcome', 'objects', 'reason', 'request_id'] as const;

/** One CSV cell, quoted only when it has to be. Formula-looking text is prefixed so spreadsheets show it as text. */
export function csvCell(value: string | number | null): string {
  if (value === null) return '';
  let text = String(value);
  if (typeof value === 'string' && /^[=+\-@\t\r]/.test(text)) text = `'${text}`;
  return /[",\r\n]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text;
}

/** The CSV text for the events, header first, with CRLF line ends. */
export function auditCsv(events: AuditEventView[]): string {
  const rows = events.map((event) =>
    [
      event.seq,
      event.at,
      event.actor_kind,
      event.actor_id,
      event.command,
      event.outcome,
      event.objects.join(' '),
      event.reason,
      event.request_id,
    ].map((cell) => csvCell(cell as string | number | null)),
  );
  return [AUDIT_CSV_HEADER.join(','), ...rows.map((row) => row.join(','))].join('\r\n') + '\r\n';
}
