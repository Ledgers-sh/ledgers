//! The bank part of the Halden Works demo (issues #26 and #48): one current
//! account on the bank ledger account, and the March statement the bank would
//! have sent for it, uploaded and imported through the ordinary path.
//!
//! The statement is derived from the book's own movements on the bank ledger
//! account, so most of its lines match posted entries and show as suggestions.
//! Two March movements are left out of it (entries the bank has not booked
//! yet), and two lines the books do not know are added (a fee and an unexplained
//! receipt), so the reconciliation shows a difference with its causes.
//!
//! Idempotent: the account is created once (found by its IBAN), the document is
//! stored once (found by its hash), and the import stores each line once.

use crate::demo::{DEMO_OWNER_ID, OwnerAuth, run_as_person};
use crate::dispatch::Deps;
use ledgers_bank::model::StatementsImportInput;
use ledgers_bank::parse;
use ledgers_bank::statement::Format;
use ledgers_demo::{DemoBook, EntryKind, Side};
use std::fmt::Write as _;

/// The demo company's bank ledger account (chart number).
const BANK_LEDGER: &str = "1020";
/// The Halden Works operating account. Invented, with valid check digits.
const HALDEN_IBAN: &str = "DE64100200300400500600";
/// The bank's identifier for the account. Invented.
const HALDEN_BIC: &str = "HALDDEFFXXX";
/// The statement's file name in the document store.
const FILE_NAME: &str = "halden-works-march-statement.xml";

/// What the bank seeding did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BankSeedReport {
    /// The account was created by this run.
    pub account_created: bool,
    /// The statement document was stored by this run.
    pub document_stored: bool,
    /// Lines the import stored by this run.
    pub lines_imported: u32,
}

/// One movement on the bank ledger account, signed from the bank's side.
#[derive(Clone, Debug)]
struct Movement {
    date: String,
    minor: i64,
    description: String,
    key: String,
}

/// The movements on the bank ledger account, with reversed pairs netted out
/// (both sides of a reversal leave the bank untouched).
fn movements(book: &DemoBook) -> Vec<Movement> {
    let reversed: std::collections::BTreeSet<&str> = book
        .entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            EntryKind::Reverse { of } => Some(of.as_str()),
            EntryKind::Post { .. } => None,
        })
        .collect();
    let mut out = Vec::new();
    for entry in &book.entries {
        let EntryKind::Post { lines } = &entry.kind else {
            continue;
        };
        if reversed.contains(entry.external_id.as_str()) {
            continue;
        }
        for line in lines.iter().filter(|line| line.account == BANK_LEDGER) {
            let minor = match line.side {
                Side::Debit => line.minor,
                Side::Credit => -line.minor,
            };
            out.push(Movement {
                date: entry.posting_date.clone(),
                minor,
                description: entry.description.clone(),
                key: entry.external_id.clone(),
            });
        }
    }
    out.sort_by(|a, b| a.date.cmp(&b.date).then(a.key.cmp(&b.key)));
    out
}

/// A minor-unit amount as the decimal text an account takes (`1234.50`).
fn decimal(minor: i64) -> String {
    let sign = if minor < 0 { "-" } else { "" };
    let magnitude = minor.unsigned_abs();
    format!("{sign}{}.{:02}", magnitude / 100, magnitude % 100)
}

/// Escapes text for an XML element.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The March statement as a camt.053 document, built from the movements.
fn statement_xml(book: &DemoBook) -> (String, i64) {
    let year = book.year;
    let march_start = format!("{year}-03-01");
    let march_end = format!("{year}-03-31");
    let all = movements(book);
    let opening: i64 = all
        .iter()
        .filter(|m| m.date < march_start)
        .map(|m| m.minor)
        .sum();
    let march: Vec<&Movement> = all
        .iter()
        .filter(|m| m.date >= march_start && m.date <= march_end)
        .collect();
    // Two of the month's movements are not on the statement yet.
    let on_statement = march.len().saturating_sub(2);
    let mut lines: Vec<(String, i64, String, String)> = march[..on_statement]
        .iter()
        .map(|m| {
            (
                m.date.clone(),
                m.minor,
                m.description.clone(),
                format!("{}-{}", m.key, m.minor),
            )
        })
        .collect();
    lines.push((
        format!("{year}-03-18"),
        48_000,
        "Zahlung ohne Referenz".to_owned(),
        "bank-only-receipt".to_owned(),
    ));
    lines.push((
        format!("{year}-03-27"),
        -490,
        "Kontofuehrungsentgelt Maerz".to_owned(),
        "bank-only-fee".to_owned(),
    ));
    lines.sort_by(|a, b| a.0.cmp(&b.0).then(a.3.cmp(&b.3)));
    let closing: i64 = opening + lines.iter().map(|l| l.1).sum::<i64>();
    let mut xml = String::new();
    let _ = writeln!(xml, r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    let _ = writeln!(
        xml,
        r#"<Document xmlns="urn:iso:std:iso:20022:tech:xsd:camt.053.001.08">"#
    );
    let _ = writeln!(xml, "  <BkToCstmrStmt>");
    let _ = writeln!(xml, "    <Stmt>");
    let _ = writeln!(xml, "      <Id>HALDEN-{year}-03</Id>");
    let _ = writeln!(
        xml,
        "      <Acct><Id><IBAN>{HALDEN_IBAN}</IBAN></Id><Ccy>EUR</Ccy>"
    );
    let _ = writeln!(
        xml,
        "        <Svcr><FinInstnId><BIC>{HALDEN_BIC}</BIC></FinInstnId></Svcr></Acct>"
    );
    let _ = writeln!(
        xml,
        "      <Bal><Tp><CdOrPrtry><Cd>OPBD</Cd></CdOrPrtry></Tp><Amt Ccy=\"EUR\">{}</Amt><CdtDbtInd>{}</CdtDbtInd><Dt><Dt>{march_start}</Dt></Dt></Bal>",
        decimal(opening.abs()),
        if opening < 0 { "DBIT" } else { "CRDT" }
    );
    for (index, (date, minor, text, _key)) in lines.iter().enumerate() {
        let indicator = if *minor < 0 { "DBIT" } else { "CRDT" };
        let _ = writeln!(xml, "      <Ntry>");
        let _ = writeln!(
            xml,
            "        <Amt Ccy=\"EUR\">{}</Amt><CdtDbtInd>{indicator}</CdtDbtInd>",
            decimal(minor.abs())
        );
        let _ = writeln!(
            xml,
            "        <Sts><Cd>BOOK</Cd></Sts><BookgDt><Dt>{date}</Dt></BookgDt><ValDt><Dt>{date}</Dt></ValDt>"
        );
        let _ = writeln!(
            xml,
            "        <AcctSvcrRef>HW{year}03{index:04}</AcctSvcrRef>"
        );
        let _ = writeln!(
            xml,
            "        <NtryDtls><TxDtls><RmtInf><Ustrd>{}</Ustrd></RmtInf></TxDtls></NtryDtls>",
            escape(text)
        );
        let _ = writeln!(xml, "      </Ntry>");
    }
    let _ = writeln!(
        xml,
        "      <Bal><Tp><CdOrPrtry><Cd>CLBD</Cd></CdOrPrtry></Tp><Amt Ccy=\"EUR\">{}</Amt><CdtDbtInd>{}</CdtDbtInd><Dt><Dt>{march_end}</Dt></Dt></Bal>",
        decimal(closing.abs()),
        if closing < 0 { "DBIT" } else { "CRDT" }
    );
    let _ = writeln!(xml, "    </Stmt>");
    let _ = writeln!(xml, "  </BkToCstmrStmt>");
    let _ = writeln!(xml, "</Document>");
    (xml, opening)
}

/// Seeds the bank account and its March statement for the demo book.
///
/// # Errors
/// A message naming the step that failed.
#[allow(clippy::single_match_else, clippy::too_many_lines)]
pub async fn seed_bank(
    deps: &Deps,
    company_id: &str,
    book: &DemoBook,
) -> Result<BankSeedReport, String> {
    let mut report = BankSeedReport::default();
    // The statement and account are Halden Works': other demo books have their own banks.
    if book.company != ledgers_demo::halden::COMPANY {
        return Ok(report);
    }
    let db = deps.db.as_ref();
    let (xml, opening) = statement_xml(book);
    let account = match ledgers_bank::store::accounts(db, company_id, true)
        .await
        .map_err(|err| format!("read bank accounts: {err}"))?
        .into_iter()
        .find(|a| a.iban == HALDEN_IBAN)
    {
        Some(existing) => existing,
        None => {
            let input = ledgers_bank::model::AccountsCreateInput {
                name: "Halden Works Girokonto".to_owned(),
                iban: HALDEN_IBAN.to_owned(),
                bic: Some(HALDEN_BIC.to_owned()),
                currency: book.currency.to_owned(),
                ledger_account_number: BANK_LEDGER.to_owned(),
                opening_amount: decimal(opening),
                opening_date: format!("{}-03-01", book.year),
                csv_mapping: None,
                actor: DEMO_OWNER_ID.to_owned(),
            };
            let auth = OwnerAuth {
                person_id: DEMO_OWNER_ID.to_owned(),
            };
            run_as_person(
                deps,
                company_id,
                &ledgers_bank::commands::AccountsCreate,
                auth,
                &input,
            )
            .await
            .map_err(|err| format!("create bank account: {err}"))?;
            report.account_created = true;
            ledgers_bank::store::accounts(db, company_id, true)
                .await
                .map_err(|err| format!("read bank accounts: {err}"))?
                .into_iter()
                .find(|a| a.iban == HALDEN_IBAN)
                .ok_or_else(|| "the bank account was not stored".to_owned())?
        }
    };
    let bytes = xml.into_bytes();
    let sha = ledgers_bank::fingerprint::sha256_hex(&String::from_utf8_lossy(&bytes));
    let document_id = match ledgers_documents::store::find_by_hash(db, company_id, &sha)
        .await
        .map_err(|err| format!("read documents: {err}"))?
    {
        Some(document) => document.id,
        None => {
            let blob = deps
                .blob
                .as_ref()
                .ok_or_else(|| "document storage is not configured".to_owned())?;
            blob.put(&crate::documents::blob_key(&sha), &bytes, "application/xml")
                .await
                .map_err(|_| "document storage failed".to_owned())?;
            let id = deps.id_gen.ulid();
            let input = ledgers_documents::commands::RegisterInput {
                id: id.clone(),
                sha256: sha.clone(),
                size_bytes: i64::try_from(bytes.len()).unwrap_or(i64::MAX),
                mime: "application/xml".to_owned(),
                original_name: FILE_NAME.to_owned(),
                uploaded_by: format!("person:{DEMO_OWNER_ID}"),
                text_layer: Some(String::from_utf8_lossy(&bytes).into_owned()),
                retention_class: None,
                source: None,
            };
            let auth = OwnerAuth {
                person_id: DEMO_OWNER_ID.to_owned(),
            };
            run_as_person(
                deps,
                company_id,
                &ledgers_documents::commands::DocumentsRegister,
                auth,
                &input,
            )
            .await
            .map_err(|err| format!("store statement: {err}"))?;
            report.document_stored = true;
            id
        }
    };
    let statements = parse::parse(Format::Camt053, &bytes, None, book.currency)
        .map_err(|err| format!("read statement: {err}"))?;
    let import = StatementsImportInput {
        account_id: account.id.clone(),
        document_id,
        statements,
        actor: format!("person:{DEMO_OWNER_ID}"),
    };
    let auth = OwnerAuth {
        person_id: DEMO_OWNER_ID.to_owned(),
    };
    let before = ledgers_bank::store::group_counts(db, company_id, &account.id)
        .await
        .map_err(|err| format!("count lines: {err}"))?
        .values()
        .sum::<u32>();
    run_as_person(
        deps,
        company_id,
        &ledgers_bank::commands::StatementsImport,
        auth,
        &import,
    )
    .await
    .map_err(|err| format!("import statement: {err}"))?;
    let after = ledgers_bank::store::group_counts(db, company_id, &account.id)
        .await
        .map_err(|err| format!("count lines: {err}"))?
        .values()
        .sum::<u32>();
    report.lines_imported = after.saturating_sub(before);
    Ok(report)
}
