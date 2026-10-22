import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from 'preact/hooks';
import { formatPeriod, intlLocale, t, type MessageKey } from '../i18n';
import { Icon } from './icons';
import {
  monthIndexOf,
  monthsIn,
  presetOf,
  presetRange,
  type MonthRange,
  type PresetId,
} from '../lib/trends';
import { gridStep } from '../lib/combobox';

const PRESETS: readonly Exclude<PresetId, 'custom'>[] = ['thisMonth', 'lastMonth', 'thisQuarter', 'thisYear', 'last12'];

const PRESET_KEYS: Record<PresetId, MessageKey> = {
  thisMonth: 'period.preset.this_month',
  lastMonth: 'period.preset.last_month',
  thisQuarter: 'period.preset.this_quarter',
  thisYear: 'period.preset.this_year',
  last12: 'period.preset.last_12',
  custom: 'period.preset.custom',
};

/** A "YYYY-MM" key as its year and zero-based month. */
function parts(period: string): { year: number; month: number } {
  return { year: Number(period.slice(0, 4)), month: Number(period.slice(5, 7)) - 1 };
}

/** Short month names in the reader's language and numerals (Western digits stay in Arabic). */
function monthName(month: number, year: number): string {
  return new Intl.DateTimeFormat(intlLocale(), { month: 'short', timeZone: 'UTC' }).format(new Date(Date.UTC(year, month, 1)));
}

/** The text on the trigger: the range, or one month. */
export function rangeLabel(range: MonthRange): string {
  return range.from === range.to ? formatPeriod(range.from) : `${formatPeriod(range.from)} – ${formatPeriod(range.to)}`;
}

/**
 * The period selector in the top bar. A popover with presets (this month,
 * last month, this quarter, this year, the last twelve months) and a custom
 * range picked on two month grids. Keyboard: arrows move through the presets
 * and the grids, Enter or Space chooses, Page Up and Page Down change the
 * year, Escape closes and returns focus to the trigger. Tab stays inside while
 * it is open; a click outside closes it. On phones it is a bottom sheet.
 */
export function PeriodPicker({ range, onChange }: { range: MonthRange; onChange: (range: MonthRange) => void }) {
  const [open, setOpen] = useState(false);
  const [today, setToday] = useState(() => new Date());
  const [placement, setPlacement] = useState<'down' | 'up'>('down');
  const reactId = useId().replace(/:/g, '');
  const wrapRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const popRef = useRef<HTMLDivElement>(null);
  const titleId = `pp-title-${reactId}`;
  const preset = presetOf(range, today);
  const [fromYear, setFromYear] = useState(() => parts(range.from).year);
  const [toYear, setToYear] = useState(() => parts(range.to).year);
  const [focusMonth, setFocusMonth] = useState<{ grid: 'from' | 'to'; index: number } | null>(null);
  const inRange = useMemo(() => new Set(monthsIn(range)), [range]);

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
    const pop = popRef.current;
    const trigger = triggerRef.current;
    if (pop && trigger) {
      const rect = trigger.getBoundingClientRect();
      const room = window.innerHeight - rect.bottom;
      setPlacement(room < pop.offsetHeight + 12 && rect.top > room ? 'up' : 'down');
    }
    const first = pop?.querySelector<HTMLElement>('[role="radio"][aria-checked="true"]') ?? pop?.querySelector<HTMLElement>('button');
    first?.focus();
  }, [open]);

  const openPicker = () => {
    setToday(new Date());
    setFromYear(parts(range.from).year);
    setToYear(parts(range.to).year);
    setOpen(true);
  };

  const close = () => {
    setOpen(false);
    triggerRef.current?.focus();
  };

  const pick = (next: MonthRange) => {
    onChange(next);
  };

  const pickMonth = (grid: 'from' | 'to', period: string) => {
    if (grid === 'from') {
      pick({ from: period, to: period > range.to ? period : range.to });
    } else {
      pick({ from: period < range.from ? period : range.from, to: period });
    }
  };

  const onDialogKey = (event: KeyboardEvent) => {
    if (event.key === 'Escape') {
      event.preventDefault();
      close();
      return;
    }
    if (event.key === 'Tab') {
      const pop = popRef.current;
      if (!pop) return;
      const focusable = [...pop.querySelectorAll<HTMLElement>('button:not([disabled])')].filter((el) => el.tabIndex >= 0);
      if (focusable.length === 0) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    }
  };

  const rtl = typeof document !== 'undefined' && document.documentElement.dir === 'rtl';

  const onPresetKey = (event: KeyboardEvent, index: number) => {
    const step = event.key === 'ArrowDown' || event.key === 'ArrowRight' ? 1 : event.key === 'ArrowUp' || event.key === 'ArrowLeft' ? -1 : 0;
    if (!step) return;
    event.preventDefault();
    const next = (index + step + PRESETS.length + 1) % (PRESETS.length + 1);
    const buttons = popRef.current?.querySelectorAll<HTMLButtonElement>('[role="radio"]');
    buttons?.[next]?.focus();
    if (next < PRESETS.length) pick(presetRange(PRESETS[next], today));
  };

  const onMonthKey = (event: KeyboardEvent, grid: 'from' | 'to', index: number) => {
    if (event.key === 'PageDown' || event.key === 'PageUp') {
      event.preventDefault();
      const direction = event.key === 'PageDown' ? 1 : -1;
      if (grid === 'from') setFromYear((value) => value + direction);
      else setToYear((value) => value + direction);
      return;
    }
    const target = gridStep(index, 12, 3, event.key, rtl);
    if (target === index) return;
    event.preventDefault();
    setFocusMonth({ grid, index: target });
    popRef.current?.querySelector<HTMLElement>(`[data-grid="${grid}"][data-month="${target}"]`)?.focus();
  };

  const yearGrid = (grid: 'from' | 'to', year: number, setYear: (value: number) => void) => {
    const selected = parts(grid === 'from' ? range.from : range.to);
    const tabStop = focusMonth?.grid === grid ? focusMonth.index : selected.year === year ? selected.month : 0;
    const label = grid === 'from' ? t('period.from') : t('period.to');
    return (
      <div class="pp-grid-wrap">
        <div class="pp-year">
          <button type="button" class="btn btn-ghost btn-sm pp-step" aria-label={t('period.prev_year')} onClick={() => setYear(year - 1)}>
            <Icon name={rtl ? 'chevron-right' : 'chevron-left'} />
          </button>
          <span class="pp-year-value" aria-live="polite">
            {label} · {year}
          </span>
          <button type="button" class="btn btn-ghost btn-sm pp-step" aria-label={t('period.next_year')} onClick={() => setYear(year + 1)}>
            <Icon name={rtl ? 'chevron-left' : 'chevron-right'} />
          </button>
        </div>
        <div class="pp-months" role="group" aria-label={`${label} ${year}`}>
          {Array.from({ length: 12 }, (_, month) => {
            const period = `${year}-${String(month + 1).padStart(2, '0')}`;
            const isEdge = period === range.from || period === range.to;
            const isSelected = period === (grid === 'from' ? range.from : range.to);
            const isCurrent = monthIndexOf(today) === year * 12 + month;
            const within = inRange.has(period);
            return (
              <button
                type="button"
                key={month}
                data-grid={grid}
                data-month={month}
                class={`pp-month${within ? ' within' : ''}${isEdge ? ' edge' : ''}${isCurrent ? ' now' : ''}`}
                aria-pressed={isSelected ? 'true' : 'false'}
                aria-label={`${monthName(month, year)} ${year}`}
                tabIndex={tabStop === month ? 0 : -1}
                onFocus={() => setFocusMonth({ grid, index: month })}
                onClick={() => pickMonth(grid, period)}
                onKeyDown={(event) => onMonthKey(event as KeyboardEvent, grid, month)}
              >
                {monthName(month, year)}
              </button>
            );
          })}
        </div>
      </div>
    );
  };

  const selectedPreset: PresetId = preset;

  return (
    <div class="pp" ref={wrapRef}>
      <button
        ref={triggerRef}
        type="button"
        class="pp-trigger"
        aria-haspopup="dialog"
        aria-expanded={open ? 'true' : 'false'}
        aria-controls={open ? `pp-pop-${reactId}` : undefined}
        aria-label={`${t('shell.period')}: ${t(PRESET_KEYS[preset])}, ${rangeLabel(range)}`}
        onClick={() => (open ? close() : openPicker())}
      >
        <span class="pp-label" aria-hidden="true">
          {t('shell.period')}
        </span>
        <span class="pp-value" aria-hidden="true">
          {rangeLabel(range)}
        </span>
        <Icon name="chevron" class="pp-caret" />
      </button>
      {open && (
        <div
          id={`pp-pop-${reactId}`}
          ref={popRef}
          class={`pp-pop ${placement}`}
          role="dialog"
          aria-modal="false"
          aria-labelledby={titleId}
          onKeyDown={(event) => onDialogKey(event as KeyboardEvent)}
        >
          <div class="pp-head">
            <h2 id={titleId}>{t('period.title')}</h2>
            <button type="button" class="btn btn-ghost btn-sm pp-close" aria-label={t('period.close')} onClick={close}>
              <Icon name="close" />
            </button>
          </div>
          <div class="pp-presets" role="radiogroup" aria-label={t('period.presets')}>
            {[...PRESETS, 'custom' as const].map((id, index) => {
              const checked = selectedPreset === id;
              return (
                <button
                  type="button"
                  key={id}
                  role="radio"
                  aria-checked={checked ? 'true' : 'false'}
                  tabIndex={checked || (selectedPreset === 'custom' && id === 'custom') ? 0 : -1}
                  class={`pp-preset${checked ? ' on' : ''}`}
                  onClick={() => {
                    if (id !== 'custom') pick(presetRange(id, today));
                  }}
                  onKeyDown={(event) => onPresetKey(event as KeyboardEvent, index)}
                >
                  <span>{t(PRESET_KEYS[id])}</span>
                  {checked && <Icon name="check" class="pp-check" />}
                </button>
              );
            })}
          </div>
          <div class="pp-custom">
            <p class="pp-custom-label">{t('period.custom_hint')}</p>
            <div class="pp-grids">
              {yearGrid('from', fromYear, setFromYear)}
              {yearGrid('to', toYear, setToYear)}
            </div>
          </div>
          <div class="pp-foot">
            <span class="pp-summary" aria-live="polite">
              {t('period.selected', { range: rangeLabel(range) })}
            </span>
            <button type="button" class="btn btn-primary btn-sm" onClick={close}>
              {t('period.done')}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
