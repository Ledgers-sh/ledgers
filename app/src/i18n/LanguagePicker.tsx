import { useEffect, useRef, useState } from 'preact/hooks';
import { LOCALES, setLocale, t, useLocale, type LocaleInfo } from './index';

/**
 * Searchable language picker for the top bar. The button shows the language's
 * own name; the popup is a combobox (search field plus listbox) that works by
 * keyboard: type to filter, arrows to move, Enter to choose, Escape to close.
 */
export function LanguagePicker() {
  const current = useLocale();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(0);
  const wrapRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  const needle = query.trim().toLowerCase();
  const options: LocaleInfo[] = needle
    ? LOCALES.filter((locale) => locale.name.toLowerCase().includes(needle) || locale.code.includes(needle))
    : [...LOCALES];
  const activeIndex = Math.min(active, Math.max(options.length - 1, 0));

  useEffect(() => {
    if (!open) return;
    inputRef.current?.focus();
    const onPointer = (event: Event) => {
      if (wrapRef.current && !wrapRef.current.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener('mousedown', onPointer);
    return () => document.removeEventListener('mousedown', onPointer);
  }, [open]);

  const close = () => {
    setOpen(false);
    setQuery('');
    setActive(0);
    buttonRef.current?.focus();
  };

  const choose = (locale: LocaleInfo) => {
    setLocale(locale.code);
    close();
  };

  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key === 'ArrowDown') {
      event.preventDefault();
      setActive((index) => Math.min(index + 1, Math.max(options.length - 1, 0)));
    } else if (event.key === 'ArrowUp') {
      event.preventDefault();
      setActive((index) => Math.max(index - 1, 0));
    } else if (event.key === 'Home') {
      event.preventDefault();
      setActive(0);
    } else if (event.key === 'End') {
      event.preventDefault();
      setActive(Math.max(options.length - 1, 0));
    } else if (event.key === 'Enter') {
      event.preventDefault();
      if (options[activeIndex]) choose(options[activeIndex]);
    } else if (event.key === 'Escape') {
      event.preventDefault();
      close();
    }
  };

  return (
    <div class="lang" ref={wrapRef}>
      <button
        ref={buttonRef}
        type="button"
        class="btn btn-ghost btn-sm lang-button"
        aria-haspopup="listbox"
        aria-expanded={open ? 'true' : 'false'}
        aria-controls={open ? 'lang-popup' : undefined}
        aria-label={`${t('language.label')}: ${current.name}`}
        onClick={() => {
          setOpen((value) => !value);
          setActive(0);
        }}
      >
        <span lang={current.code} dir={current.dir}>
          {current.name}
        </span>
        <span aria-hidden="true" class="lang-caret">
          ▾
        </span>
      </button>
      {open && (
        <div id="lang-popup" class="lang-popup">
          <input
            ref={inputRef}
            class="input lang-search"
            type="text"
            role="combobox"
            aria-expanded="true"
            aria-controls="lang-list"
            aria-autocomplete="list"
            aria-label={t('language.search_label')}
            aria-activedescendant={options[activeIndex] ? `lang-opt-${options[activeIndex].code}` : undefined}
            placeholder={t('language.search_placeholder')}
            autoComplete="off"
            spellcheck={false}
            value={query}
            onInput={(event) => {
              setQuery((event.target as HTMLInputElement).value);
              setActive(0);
            }}
            onKeyDown={onKeyDown}
          />
          <ul id="lang-list" class="lang-list" role="listbox" aria-label={t('language.label')}>
            {options.map((locale, index) => (
              <li
                key={locale.code}
                id={`lang-opt-${locale.code}`}
                role="option"
                aria-selected={locale.code === current.code ? 'true' : 'false'}
                class={`lang-option${index === activeIndex ? ' active' : ''}`}
                lang={locale.code}
                dir={locale.dir}
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => choose(locale)}
              >
                {locale.name}
              </li>
            ))}
          </ul>
          {options.length === 0 && <p class="muted lang-none">{t('language.none', { query })}</p>}
        </div>
      )}
    </div>
  );
}
