/** Types for the generator, so the TypeScript tests can import it. See gen-i18n.mjs. */
export type Catalogue = Record<string, string>;
export function parseCatalogue(text: string, source: string): Catalogue;
export function renderChartNames(catalogues: Record<string, Catalogue>): string;
export function loadCatalogues(templatesDir: string): Record<string, Catalogue>;
export const OUTPUT: string;
export const TEMPLATES: string;
