import { describe, expect, it } from 'vitest';
import { auditCsv, csvCell } from './audit';
import type { AuditEventView } from '../api/settings';

const event: AuditEventView = {
  id: '01A',
  seq: 7,
  at: '2026-10-22T10:00:00Z',
  actor_kind: 'agent',
  actor_id: 'key-1',
  command: 'entries.post',
  outcome: 'denied',
  objects: ['J1', '2026'],
  reason: 'over the limit, "per entry"',
  request_id: 'req-9',
};

describe('audit CSV', () => {
  it('quotes cells that hold commas, quotes or line breaks', () => {
    expect(csvCell('a,b')).toBe('"a,b"');
    expect(csvCell('say "hi"')).toBe('"say ""hi"""');
    expect(csvCell('line\nbreak')).toBe('"line\nbreak"');
    expect(csvCell(null)).toBe('');
    expect(csvCell(42)).toBe('42');
  });

  it('neutralizes spreadsheet formulas', () => {
    expect(csvCell('=HYPERLINK("x")')).toBe(`"'=HYPERLINK(""x"")"`);
    expect(csvCell('@sum')).toBe("'@sum");
  });

  it('writes a header and one row per event with CRLF ends', () => {
    const csv = auditCsv([event]);
    const lines = csv.split('\r\n');
    expect(lines[0]).toBe('seq,at,actor_kind,actor_id,command,outcome,objects,reason,request_id');
    expect(lines[1]).toBe('7,2026-10-22T10:00:00Z,agent,key-1,entries.post,denied,J1 2026,"over the limit, ""per entry""",req-9');
    expect(lines[2]).toBe('');
  });

  it('an empty list is the header alone', () => {
    expect(auditCsv([])).toBe('seq,at,actor_kind,actor_id,command,outcome,objects,reason,request_id\r\n');
  });
});
