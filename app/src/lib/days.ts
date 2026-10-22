/**
 * Day helpers for the single-day picker. Days are "YYYY-MM-DD" in UTC, the
 * same days the server filters on, so a picked day never shifts with the
 * reader's time zone.
 */

const DAY = /^(\d{4})-(\d{2})-(\d{2})$/;

/** The "YYYY-MM-DD" text of a UTC calendar day; `monthIndex` is zero-based and may overflow. */
export function isoDay(year: number, monthIndex: number, day: number): string {
  const date = new Date(Date.UTC(year, monthIndex, day));
  return `${date.getUTCFullYear()}-${String(date.getUTCMonth() + 1).padStart(2, '0')}-${String(date.getUTCDate()).padStart(2, '0')}`;
}

/** Whether the text is a real "YYYY-MM-DD" day. */
export function isDay(value: string): boolean {
  const match = DAY.exec(value);
  if (!match) return false;
  return isoDay(Number(match[1]), Number(match[2]) - 1, Number(match[3])) === value;
}

/** The day `count` days after `day` (negative goes back). */
export function shiftDay(day: string, count: number): string {
  const match = DAY.exec(day);
  if (!match) return day;
  return isoDay(Number(match[1]), Number(match[2]) - 1, Number(match[3]) + count);
}

/** The same day-of-month `count` months after `day`, clamped to the last day of a shorter month. */
export function shiftMonth(day: string, count: number): string {
  const match = DAY.exec(day);
  if (!match) return day;
  const year = Number(match[1]);
  const month = Number(match[2]) - 1 + count;
  const lastOfTarget = new Date(Date.UTC(year, month + 1, 0)).getUTCDate();
  return isoDay(year, month, Math.min(Number(match[3]), lastOfTarget));
}

/** Today as a UTC "YYYY-MM-DD" day. */
export function todayUtc(now: Date = new Date()): string {
  return isoDay(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate());
}

/**
 * The cells of one month as a Monday-first grid of whole weeks: a day's text,
 * or null for the blank cells before the first day and after the last.
 */
export function monthGrid(day: string): (string | null)[] {
  const match = DAY.exec(day);
  if (!match) return [];
  const year = Number(match[1]);
  const monthIndex = Number(match[2]) - 1;
  const lead = (new Date(Date.UTC(year, monthIndex, 1)).getUTCDay() + 6) % 7;
  const length = new Date(Date.UTC(year, monthIndex + 1, 0)).getUTCDate();
  const cells: (string | null)[] = Array.from({ length: lead }, () => null);
  for (let date = 1; date <= length; date += 1) cells.push(isoDay(year, monthIndex, date));
  while (cells.length % 7 !== 0) cells.push(null);
  return cells;
}
