import { useEffect, useId, useLayoutEffect, useRef, useState } from 'preact/hooks';
import { formatDate, intlLocale, t } from '../i18n';
import { isDay, monthGrid, shiftDay, shiftMonth, todayUtc } from '../lib/days';
import { Icon } from './icons';

/** The weekday names of one Monday-first week, in the reader's language. */
function weekdayNames(): string[] {
  // 1 January 2024 was a Monday.
  return Array.from({ length: 7 }, (_, index) =>
    new Intl.DateTimeFormat(intlLocale(), { weekday: 'short', timeZone: 'UTC' }).format(new Date(Date.UTC(2024, 0, 1 + index))),
  );
}

/** The month and year of a day, as the grid's heading. */
function monthTitle(day: string): string {
  return new Intl.DateTimeFormat(intlLocale(), { month: 'long', year: 'numeric', timeZone: 'UTC' }).format(new Date(`${day}T00:00:00Z`));
}

/**
 * A single-day field in the period picker's style: a trigger that shows the
 * day, and a popover with a month grid. A day chosen closes it. Keyboard: the
 * arrows move one day or one week, Page Up and Page Down move one month,
 * Enter or Space chooses, and Escape closes and returns focus to the trigger.
 * Days are UTC, the days the server filters on. "Clear" leaves the field empty.
 */
export function DatePicker({ label, value, onChange }: { label: string; value: string; onChange: (value: string) => void }) {
  const [open, setOpen] = useState(false);
  // The day the grid shows and the keyboard starts from: the chosen day, else today.
  const [anchor, setAnchor] = useState(() => (isDay(value) ? value : todayUtc()));
  const reactId = useId().replace(/:/g, '');
  const wrapRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const popRef = useRef<HTMLDivElement>(null);
  const movedByKey = useRef(false);
  const titleId = `dp-title-${reactId}`;
  const today = todayUtc();
  const rtl = typeof document !== 'undefined' && document.documentElement.dir === 'rtl';
  const shown = isDay(value) ? formatDate(value) : t('settings.any');

  useEffect(() => {
    if (!open) return;
    const onPointer = (event: Event) => {
      if (wrapRef.current && !wrapRef.current.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener('pointerdown', onPointer);
    return () => document.removeEventListener('pointerdown', onPointer);
  }, [open]);

  useLayoutEffect(() => {
    if (!open) return;
    const first = popRef.current?.querySelector<HTMLElement>('[data-day][tabindex="0"]') ?? popRef.current?.querySelector<HTMLElement>('button');
    first?.focus();
  }, [open]);

  // A key moved the anchor: the grid may now show another month, so focus the day it lands on.
  useLayoutEffect(() => {
    if (!movedByKey.current) return;
    movedByKey.current = false;
    popRef.current?.querySelector<HTMLElement>(`[data-day="${anchor}"]`)?.focus();
  }, [anchor]);

  const openPicker = () => {
    setAnchor(isDay(value) ? value : todayUtc());
    setOpen(true);
  };

  const close = () => {
    setOpen(false);
    triggerRef.current?.focus();
  };

  const pick = (day: string) => {
    onChange(day);
    close();
  };

  const moveTo = (day: string) => {
    movedByKey.current = true;
    setAnchor(day);
  };

  const onDayKey = (event: KeyboardEvent, day: string) => {
    const across = rtl ? -1 : 1;
    const steps: Record<string, string> = {
      ArrowLeft: shiftDay(day, -across),
      ArrowRight: shiftDay(day, across),
      ArrowUp: shiftDay(day, -7),
      ArrowDown: shiftDay(day, 7),
      PageUp: shiftMonth(day, -1),
      PageDown: shiftMonth(day, 1),
    };
    const next = steps[event.key];
    if (next === undefined) return;
    event.preventDefault();
    moveTo(next);
  };

  const weekdays = weekdayNames();
  const cells = monthGrid(anchor);
  const tabStop = isDay(value) && value.slice(0, 7) === anchor.slice(0, 7) ? value : anchor;

  return (
    <div class="pp dp" ref={wrapRef}>
      <button
        ref={triggerRef}
        type="button"
        class="pp-trigger dp-trigger"
        aria-haspopup="dialog"
        aria-expanded={open ? 'true' : 'false'}
        aria-controls={open ? `dp-pop-${reactId}` : undefined}
        aria-label={`${label}: ${shown}`}
        onClick={() => (open ? close() : openPicker())}
      >
        <span class="pp-value" aria-hidden="true">
          {shown}
        </span>
        <Icon name="chevron" class="pp-caret" />
      </button>
      {open && (
        <div
          id={`dp-pop-${reactId}`}
          ref={popRef}
          class="pp-pop down dp-pop"
          role="dialog"
          aria-modal="false"
          aria-labelledby={titleId}
          onKeyDown={(event) => {
            if (event.key === 'Escape') {
              event.preventDefault();
              close();
            }
          }}
        >
          <div class="pp-head">
            <button
              type="button"
              class="btn btn-ghost btn-sm pp-step"
              aria-label={t('date.prev_month')}
              onClick={() => setAnchor(shiftMonth(anchor, -1))}
            >
              <Icon name={rtl ? 'chevron-right' : 'chevron-left'} />
            </button>
            <h2 id={titleId} class="dp-title" aria-live="polite">
              {monthTitle(anchor)}
            </h2>
            <button
              type="button"
              class="btn btn-ghost btn-sm pp-step"
              aria-label={t('date.next_month')}
              onClick={() => setAnchor(shiftMonth(anchor, 1))}
            >
              <Icon name={rtl ? 'chevron-left' : 'chevron-right'} />
            </button>
            <button type="button" class="btn btn-ghost btn-sm pp-close" aria-label={t('period.close')} onClick={close}>
              <Icon name="close" />
            </button>
          </div>
          <div class="dp-days-wrap" role="group" aria-label={t('date.dialog')}>
            <div class="dp-week" aria-hidden="true">
              {weekdays.map((name) => (
                <span key={name}>{name}</span>
              ))}
            </div>
            <div class="dp-days">
              {cells.map((cell, index) =>
                cell === null ? (
                  <span key={`blank-${index}`} class="dp-blank" aria-hidden="true" />
                ) : (
                  <button
                    type="button"
                    key={cell}
                    data-day={cell}
                    class={`pp-month dp-day${cell === value ? ' edge' : ''}${cell === today ? ' now' : ''}`}
                    aria-pressed={cell === value ? 'true' : 'false'}
                    aria-label={formatDate(cell)}
                    tabIndex={cell === tabStop ? 0 : -1}
                    onClick={() => pick(cell)}
                    onKeyDown={(event) => onDayKey(event as KeyboardEvent, cell)}
                  >
                    {Number(cell.slice(8))}
                  </button>
                ),
              )}
            </div>
          </div>
          <div class="pp-foot">
            <button
              type="button"
              class="btn btn-ghost btn-sm"
              onClick={() => {
                onChange('');
                close();
              }}
            >
              {t('settings.clear_filters')}
            </button>
            <button type="button" class="btn btn-primary btn-sm" onClick={close}>
              {t('period.done')}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
