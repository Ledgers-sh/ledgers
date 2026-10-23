/** Small pieces the customers and vendors screens share. */
import { t } from '../i18n';
import type { VatStatus } from '../lib/parties';

const VAT_KEY: Record<VatStatus, 'parties.vat_valid' | 'parties.vat_invalid' | 'parties.vat_unchecked' | 'parties.vat_none'> = {
  valid: 'parties.vat_valid',
  invalid: 'parties.vat_invalid',
  unchecked: 'parties.vat_unchecked',
  none: 'parties.vat_none',
};

/** The VAT check as a chip: valid, invalid, unchecked, or no VAT ID. The text carries the meaning; the colour only repeats it. */
export function VatChip({ status }: { status: VatStatus }) {
  return <span class={`badge party-vat vat-${status}`}>{t(VAT_KEY[status])}</span>;
}
