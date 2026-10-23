# E-Invoice Test Fixtures

Official e-invoice samples for UBL 2.1, UN/CEFACT CII, and ZUGFeRD (PDF/A-3) parsing tests.
All parties and data are fictitious sample data provided by the standards bodies.

## Samples

| File | Origin | Commit SHA | Version | Licence | Profile/Syntax |
|------|--------|------------|---------|---------|-----------------|
| `xrechnung-ubl.xml` | [KoSIT xrechnung-testsuite](https://raw.githubusercontent.com/itplr-kosit/xrechnung-testsuite/44c3905c69ca236aa3b94c04be1b7e9bd066ba03/src/test/business-cases/standard/01.12a-INVOICE_ubl.xml) | 44c3905 | v2026-08-31 | Apache-2.0 | UBL 2.1, XRECHNUNG 3.0 |
| `xrechnung-cii.xml` | [KoSIT xrechnung-testsuite](https://raw.githubusercontent.com/itplr-kosit/xrechnung-testsuite/44c3905c69ca236aa3b94c04be1b7e9bd066ba03/src/test/business-cases/standard/01.12a-INVOICE_uncefact.xml) | 44c3905 | v2026-08-31 | Apache-2.0 | UN/CEFACT CII, XRECHNUNG 3.0 |
| `zugferd-extended.pdf` | [ZUGFeRD mustangproject](https://raw.githubusercontent.com/ZUGFeRD/mustangproject/996f219378c7eff83b0f18d193325d07ccb0f93a/library/src/test/resources/EXTENDED_Fremdwaehrung_wdis_fx-pdfa4.pdf) | 996f219 | 2.4.0 | Apache-2.0 | PDF/A-3, Factur-X EXTENDED profile, embedded CII |

## Hostile Corpus

The `hostile/` directory contains malformed XML and attack payloads for security testing (XXE, DOS, etc.).
