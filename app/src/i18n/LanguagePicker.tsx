import { LOCALES, setLocale, t, useLocale } from './index';
import { Select } from '../ui/Select';

/**
 * The top bar's language choice: the shared Select, each language shown in its
 * own name. Typing a letter jumps to a language; Enter chooses.
 */
export function LanguagePicker() {
  const current = useLocale();
  return (
    <Select
      id="language-select"
      class="compact lang"
      label={t('language.label')}
      value={current.code}
      searchable={false}
      options={LOCALES.map((locale) => ({ value: locale.code, label: locale.name, lang: locale.code, dir: locale.dir }))}
      onChange={(code) => {
        if (code === 'en' || code === 'de' || code === 'id' || code === 'ar') setLocale(code);
      }}
    />
  );
}
