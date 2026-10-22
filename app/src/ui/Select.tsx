import { Fragment } from 'preact';
import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from 'preact/hooks';
import { t } from '../i18n';
import { Icon } from './icons';
import { filterChoices, firstEnabled, placementFor, stepIndex, typeaheadIndex } from '../lib/combobox';

export interface SelectOption {
  value: string;
  /** The text shown and matched by search. */
  label: string;
  /** Secondary text (for example an account number), shown after the label and matched by search. */
  hint?: string;
  /** A heading the option sits under. Options with the same group are shown together, in the order given. */
  group?: string;
  disabled?: boolean;
  /** Language and direction of the label when it is not the page's (a language's own name, for example). */
  lang?: string;
  dir?: 'ltr' | 'rtl';
  /** A colour dot shown before the label (a business unit's colour, for example). Decorative. */
  swatch?: string;
  /** The shorter text the trigger shows once chosen, when the label is too long for a compact control. */
  trigger?: string;
  /** A small badge after the label (a role, for example). Decorative: its text is read with the label. */
  badge?: { label: string; className: string };
}

export interface SelectProps {
  /** Id of the trigger button; also the base for the popup's ids. */
  id: string;
  /** Accessible name when there is no visible label to point at. */
  label?: string;
  /** Id of the visible label element; the trigger is named by it. */
  labelledBy?: string;
  value: string;
  options: readonly SelectOption[];
  onChange: (value: string) => void;
  /** Form field name: a hidden input carries the value so the control submits natively. */
  name?: string;
  /** Shown on the trigger when no option matches `value`. */
  placeholder?: string;
  /** Show a search field in the popup. Defaults to on for more than seven options. */
  searchable?: boolean;
  disabled?: boolean;
  invalid?: boolean;
  describedBy?: string;
  onBlur?: () => void;
  /** Extra classes for the trigger wrapper, for example `compact`. */
  class?: string;
  /** Text for the trigger's side of the label when the value is empty. */
  emptyLabel?: string;
}

const SEARCH_THRESHOLD = 7;

/**
 * A select with a designed popup, for every choice in the app. Keyboard:
 * arrows, Home and End move; Enter or Space chooses; Escape closes and returns
 * focus to the trigger; typing a letter jumps to the next match (or filters,
 * when the list is searchable). The trigger is an ARIA combobox that owns a
 * listbox; the value also submits as a hidden form field.
 */
export function Select(props: SelectProps) {
  const { id, value, options, onChange, name, placeholder, disabled, invalid, describedBy, onBlur } = props;
  const searchable = props.searchable ?? options.length > SEARCH_THRESHOLD;
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(0);
  const [placement, setPlacement] = useState<'down' | 'up'>('down');
  const wrapRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const reactId = useId();
  const listId = `${id}-list-${reactId.replace(/:/g, '')}`;
  const selected = options.find((option) => option.value === value);

  const visible = useMemo(() => filterChoices(options, query), [options, query]);

  const activeIndex = visible.length === 0 ? -1 : Math.min(Math.max(active, 0), visible.length - 1);
  const activeOption = activeIndex >= 0 ? visible[activeIndex] : undefined;
  const optionId = (index: number) => `${id}-opt-${index}`;

  // Focus moves in the layout phase so the first key after opening already reaches the popup.
  useLayoutEffect(() => {
    if (!open) return;
    if (searchable) searchRef.current?.focus();
    else listRef.current?.focus();
  }, [open, searchable]);

  useEffect(() => {
    if (!open) return;
    const onPointer = (event: Event) => {
      if (wrapRef.current && !wrapRef.current.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener('pointerdown', onPointer);
    return () => document.removeEventListener('pointerdown', onPointer);
  }, [open, searchable]);

  useEffect(() => {
    if (open && activeIndex >= 0) {
      const element = document.getElementById(optionId(activeIndex));
      element?.scrollIntoView?.({ block: 'nearest' });
    }
  }, [open, activeIndex]);

  const show = () => {
    if (disabled) return;
    const at = options.findIndex((option) => option.value === value);
    setQuery('');
    setActive(at >= 0 ? at : 0);
    const rect = buttonRef.current?.getBoundingClientRect();
    const below = rect ? window.innerHeight - rect.bottom : 0;
    const above = rect ? rect.top : 0;
    setPlacement(placementFor({ below, above }, 300));
    setOpen(true);
  };

  const close = (returnFocus: boolean) => {
    setOpen(false);
    setQuery('');
    if (returnFocus) buttonRef.current?.focus();
  };

  const choose = (option: SelectOption | undefined) => {
    if (!option || option.disabled) return;
    onChange(option.value);
    close(true);
  };

  const move = (delta: number) => setActive((index) => Math.max(0, stepIndex(visible, index, delta)));

  const typeahead = (letter: string) => setActive((index) => Math.max(0, typeaheadIndex(visible, index, letter)));

  const onPopupKey = (event: KeyboardEvent) => {
    switch (event.key) {
      case 'ArrowDown':
        event.preventDefault();
        move(1);
        return;
      case 'ArrowUp':
        event.preventDefault();
        move(-1);
        return;
      case 'PageDown':
        event.preventDefault();
        move(5);
        return;
      case 'PageUp':
        event.preventDefault();
        move(-5);
        return;
      case 'Home':
        if (!searchable || (event.target as HTMLElement).tagName !== 'INPUT') {
          event.preventDefault();
          setActive(Math.max(0, firstEnabled(visible)));
        }
        return;
      case 'End':
        if (!searchable || (event.target as HTMLElement).tagName !== 'INPUT') {
          event.preventDefault();
          setActive(Math.max(visible.length - 1, 0));
        }
        return;
      case 'Enter':
        event.preventDefault();
        choose(activeOption);
        return;
      case ' ':
        if (!searchable) {
          event.preventDefault();
          choose(activeOption);
        }
        return;
      case 'Escape':
        event.preventDefault();
        close(true);
        return;
      case 'Tab':
        close(false);
        return;
      default:
        if (!searchable && event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey) {
          event.preventDefault();
          typeahead(event.key);
        }
    }
  };

  const onTriggerKey = (event: KeyboardEvent) => {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      if (!open) show();
    }
  };

  const triggerName = [props.labelledBy ? props.labelledBy : null, id].filter(Boolean).join(' ');
  const showValue = selected ? (selected.trigger ?? selected.label) : (props.emptyLabel ?? placeholder ?? '');

  return (
    <div class={`sel${open ? ' open' : ''}${props.class ? ` ${props.class}` : ''}`} ref={wrapRef}>
      <button
        ref={buttonRef}
        id={id}
        type="button"
        class={`input sel-trigger${invalid ? ' invalid' : ''}${selected ? '' : ' placeholder'}`}
        role="combobox"
        aria-haspopup="listbox"
        aria-expanded={open ? 'true' : 'false'}
        aria-controls={open ? listId : undefined}
        aria-labelledby={props.labelledBy ? triggerName : undefined}
        aria-label={props.labelledBy ? undefined : props.label}
        aria-invalid={invalid ? 'true' : undefined}
        aria-describedby={describedBy}
        disabled={disabled}
        onClick={() => (open ? close(true) : show())}
        onKeyDown={onTriggerKey}
        onBlur={onBlur}
      >
        <span class="sel-value" lang={selected?.lang} dir={selected?.dir}>
          {selected?.swatch && <span class="sel-swatch" style={{ background: selected.swatch }} aria-hidden="true" />}
          {showValue}
        </span>
        <span class="sel-caret" aria-hidden="true">
          <Icon name="chevron" />
        </span>
      </button>
      {name && <input type="hidden" name={name} value={value} />}
      {open && (
        <div class={`sel-popup ${placement}`} onKeyDown={(event) => onPopupKey(event as KeyboardEvent)}>
          {searchable && (
            <input
              ref={searchRef}
              class="input sel-search"
              type="text"
              role="combobox"
              aria-expanded="true"
              aria-controls={listId}
              aria-autocomplete="list"
              aria-activedescendant={activeOption ? optionId(activeIndex) : undefined}
              aria-label={t('select.search_label')}
              placeholder={t('select.search_placeholder')}
              autoComplete="off"
              spellcheck={false}
              value={query}
              onInput={(event) => {
                setQuery((event.target as HTMLInputElement).value);
                setActive(0);
              }}
            />
          )}
          <ul
            id={listId}
            ref={listRef}
            class="sel-list"
            role="listbox"
            tabIndex={searchable ? -1 : 0}
            aria-labelledby={props.labelledBy}
            aria-label={props.labelledBy ? undefined : props.label}
            aria-activedescendant={!searchable && activeOption ? optionId(activeIndex) : undefined}
          >
            {visible.map((option, index) => {
              const showGroup = option.group !== undefined && (index === 0 || visible[index - 1].group !== option.group);
              return (
                <Fragment key={option.value}>
                  {showGroup && (
                    <li role="presentation" class="sel-group">
                      {option.group}
                    </li>
                  )}
                  <li
                    id={optionId(index)}
                    role="option"
                    aria-selected={option.value === value ? 'true' : 'false'}
                    aria-disabled={option.disabled ? 'true' : undefined}
                    class={`sel-option${index === activeIndex ? ' active' : ''}${option.value === value ? ' selected' : ''}${option.disabled ? ' disabled' : ''}`}
                    onMouseMove={() => index !== activeIndex && setActive(index)}
                    onMouseDown={(event) => event.preventDefault()}
                    onClick={() => choose(option)}
                  >
                    {option.swatch && <span class="sel-swatch" style={{ background: option.swatch }} aria-hidden="true" />}
                    <span class="sel-label" lang={option.lang} dir={option.dir}>
                      {option.label}
                    </span>
                    {option.badge && <span class={option.badge.className}>{option.badge.label}</span>}
                    {option.hint && <span class="sel-hint">{option.hint}</span>}
                    <span class="sel-check" aria-hidden="true">
                      {option.value === value && <Icon name="check" />}
                    </span>
                  </li>
                </Fragment>
              );
            })}
          </ul>
          {visible.length === 0 && (
            <p class="sel-none" role="status">
              {t('select.no_match', { query })}
            </p>
          )}
        </div>
      )}
    </div>
  );
}
