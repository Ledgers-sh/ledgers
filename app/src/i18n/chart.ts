import { CHART_NAMES } from './chart-names.generated';

/** The translated chart name for a name key in one language, or undefined when the templates do not carry it. */
export function accountNameFromCatalogue(nameKey: string, locale: string): string | undefined {
  const catalogue = (CHART_NAMES as Record<string, Record<string, string>>)[locale];
  return catalogue?.[nameKey];
}
