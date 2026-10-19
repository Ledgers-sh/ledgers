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
