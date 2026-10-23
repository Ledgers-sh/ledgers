/**
 * Advisory country notes for the agreement form: the royalty withholding rate
 * a country's pack carries, and the notes the pack makes. Copied from
 * templates/countries/{sg,id,nl}/pack.toml with their sources. They are notes,
 * not tax advice; the server's rule decides the rate it books.
 */
export interface CountrySource {
  label: string;
  url: string;
}

export interface CountryNote {
  iso: string;
  /** The royalty withholding rate the pack gives for a non-resident payee, in basis points. */
  royaltyWhtBp?: number;
  /** The pack's sourced withholding entry. */
  whtSource?: CountrySource;
  /** The notes the pack makes, each keyed for the text. */
  notes: { topic: 'wht' | 'innovatiebox'; source: CountrySource }[];
}

export const COUNTRY_NOTES: readonly CountryNote[] = [
  {
    iso: 'SG',
    royaltyWhtBp: 1000,
    whtSource: {
      label: 'IRAS, Payments that are subject to withholding tax (non-resident company)',
      url: 'https://www.iras.gov.sg/taxes/withholding-tax/payments-to-non-resident-company/payments-that-are-subject-to-withholding-tax',
    },
    notes: [
      {
        topic: 'wht',
        source: {
          label: 'IRAS, Payments that are subject to withholding tax (non-resident company)',
          url: 'https://www.iras.gov.sg/taxes/withholding-tax/payments-to-non-resident-company/payments-that-are-subject-to-withholding-tax',
        },
      },
    ],
  },
  {
    iso: 'ID',
    royaltyWhtBp: 2000,
    whtSource: { label: 'DJP, PPh Pasal 26 (Kredit Pajak)', url: 'https://pajak.go.id/en/node/34286' },
    notes: [{ topic: 'wht', source: { label: 'DJP, PPh Pasal 26 (Kredit Pajak)', url: 'https://pajak.go.id/en/node/34286' } }],
  },
  {
    iso: 'NL',
    notes: [
      {
        topic: 'innovatiebox',
        source: {
          label: 'Belastingdienst, Innovatiebox',
          url: 'https://www.belastingdienst.nl/wps/wcm/connect/bldcontentnl/belastingdienst/zakelijk/winst/vennootschapsbelasting/innovatiebox',
        },
      },
    ],
  },
];

export function countryNote(iso: string): CountryNote | undefined {
  const code = iso.trim().toUpperCase();
  return COUNTRY_NOTES.find((note) => note.iso === code);
}

/** The royalty withholding rate the payer's country pack gives, or null when it gives none. */
export function packRoyaltyWhtBp(iso: string): number | null {
  return countryNote(iso)?.royaltyWhtBp ?? null;
}
