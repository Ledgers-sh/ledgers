# Data files under templates/

Licence status of every data file bundled in this directory.

| File | Content | Licence |
| ---- | ------- | ------- |
| `generic-sme.toml` | Generic small-business chart | Original Ledgers data, MIT (repository licence) |
| `de-hgb.toml` | German chart structured on HGB §266/§275 | Original Ledgers chart; §266/§275 is statute (public law, not copyrightable). The `ebilanz` fields store only E-Bilanz-Taxonomie 6.9 position identifiers, © XBRL Deutschland e.V. — use incl. commercial use permitted with copyright notice retained (https://de.xbrl.org/disclaimer/); the notice is reproduced in the top-level NOTICE |
| `mappings/de-skr03.toml` | SKR03 → de-hgb account numbers | Original Ledgers data, MIT. Bare DATEV SKR03 account numbers only; no DATEV names or texts are bundled |
| `mappings/de-skr04.toml` | SKR04 → de-hgb account numbers | Original Ledgers data, MIT. Bare DATEV SKR04 account numbers only; no DATEV names or texts are bundled |
| `i18n/en.toml` | English translations | Original Ledgers data, MIT |
| `i18n/de.toml` | German translations | Original Ledgers data, MIT |
| `i18n/id.toml` | Indonesian translations | Original Ledgers data, MIT |
| `i18n/ar.toml` | Arabic translations | Original Ledgers data, MIT |
| `countries/de/pack.toml` | Country pack | Original Ledgers data, MIT. Statutory rates cite §12 UStG; the 2026 restaurant rate change is a TODO |
| `countries/sg/pack.toml` | Country pack | Original Ledgers data, MIT. Cites IRAS (GST 9%) and ACRA; zero and history rows are TODO |
| `countries/id/pack.toml` | Country pack | Original Ledgers data, MIT. Cites PMK 131/2024 and PMK 112/2022; pre-2025 rates are TODO |
| `nl-rgs-ez.toml` | Dutch sole-trader chart on RGS 3.7 (MKB) | Original Ledgers chart. RGS referentienummers are stored as `rgs-<digits>` identifiers only; no RGS text is copied. The RGS usage terms (referentiegrootboekschema.nl) are a TODO to confirm before a public release |
| `nl-rgs-bv.toml` | Dutch private-company chart on RGS 3.7 (MKB) | As `nl-rgs-ez.toml` |
| `i18n/nl.toml` | Dutch translations | Original Ledgers data, MIT |
| `countries/nl/pack.toml` | Country pack | Original Ledgers data, MIT. Rates TODO until dated from Belastingdienst |
| `countries/gb/pack.toml` | Country pack | Original Ledgers data, MIT. Cites GOV.UK (20% from 2011-01-04); other rates TODO |
| `countries/au/pack.toml` | Country pack | Original Ledgers data, MIT. Rate TODO until dated from the ATO |
| `countries/us/pack.toml` | Country pack | Original Ledgers data, MIT. No statutory rates; EIN format cites IRS SS-4 |
| `countries/eu/pack.toml` | Country pack | Original Ledgers data, MIT. No rates shipped; all 27 member states TODO |
| `countries/generic/pack.toml` | Country pack | Original Ledgers data, MIT. No rates; the company defines its own |

## Deliberately not bundled

- **Official SKR03/SKR04 account names** (DATEV): "Nachdruck - auch
  auszugsweise - nicht gestattet"; the mapping files carry numbers only.
- **Odoo `l10n_de`** (LGPL-3): a German chart we could otherwise have
  derived from; LGPL-3 terms are incompatible with bundling into the MIT
  data set without relicensing.
- **ERPNext "verified" regional charts** (GPL-3): GPL-3 contamination risk
  for the whole data set; not used.

The SKR numbers themselves are unprotectable facts used as an
industry-standard interchange interface; every bundled number was verified
against public listings recorded in issue #5.
