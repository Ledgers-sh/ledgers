mod cii;
mod pdf;
mod ubl;
mod xml;

use crate::money::exponent_of;
use crate::schema::{ExtractedInvoice, ExtractedLine};
use ledgers_documents::{Anchor, Location};
use std::borrow::Cow;

pub const MAX_XML_BYTES: usize = 8 << 20; // Cap: PDF document upload limit
pub const MAX_DEPTH: usize = 64; // Cap: prevent stack overflow during parsing
pub const MAX_NODES: usize = 200_000; // Cap: prevent memory exhaustion
pub const MAX_LINES: usize = 10_000; // Cap: prevent allocation bombs
const UBL_INVOICE: &str = "urn:oasis:names:specification:ubl:schema:xsd:Invoice-2";
const UBL_CREDIT_NOTE: &str = "urn:oasis:names:specification:ubl:schema:xsd:CreditNote-2";
const CII: &str = "urn:un:unece:uncefact:data:standard:CrossIndustryInvoice:100";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Ubl,
    Cii,
}
#[derive(Debug)]
pub struct EInvoice {
    pub format: Format,
    pub embedded_in_pdf: bool,
    pub invoice: ExtractedInvoice,
    pub anchors: Vec<Anchor>,
}
#[derive(Debug, Clone)]
pub enum EInvoiceError {
    TooLarge,
    TooDeep,
    TooManyNodes,
    TooManyLines,
    Doctype,
    UnknownEntity,
    Malformed(String),
    NotAnEInvoice,
    Unsupported(&'static str),
    NoEmbeddedInvoice,
}

impl std::fmt::Display for EInvoiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge => f.write_str("document too large"),
            Self::TooDeep => f.write_str("nesting too deep"),
            Self::TooManyNodes => f.write_str("too many elements"),
            Self::TooManyLines => f.write_str("too many lines"),
            Self::Doctype => f.write_str("DOCTYPE not allowed"),
            Self::UnknownEntity => f.write_str("unknown entity"),
            Self::Malformed(s) => write!(f, "malformed: {s}"),
            Self::NotAnEInvoice => f.write_str("not an e-invoice"),
            Self::Unsupported(s) => write!(f, "unsupported: {s}"),
            Self::NoEmbeddedInvoice => f.write_str("no embedded invoice"),
        }
    }
}

impl std::error::Error for EInvoiceError {}
/// Detect the format of an e-invoice file.
#[must_use]
pub fn detect(bytes: &[u8]) -> Option<Format> {
    let xml = xml_of(bytes).ok()?;
    format_of(xml::Doc::parse(&xml).ok()?.root()).ok()
}
/// Parse an e-invoice file and extract structured data.
///
/// # Errors
/// Returns an `EInvoiceError` if the input is not a valid e-invoice.
pub fn parse(bytes: &[u8]) -> Result<EInvoice, EInvoiceError> {
    let xml = xml_of(bytes)?;
    let doc = xml::Doc::parse(&xml)?;
    let format = format_of(doc.root())?;
    let mut out = Out::default();
    match format {
        Format::Ubl => ubl::map(&doc, &mut out)?,
        Format::Cii => cii::map(&doc, &mut out)?,
    }
    Ok(EInvoice {
        format,
        embedded_in_pdf: matches!(xml, Cow::Owned(_)),
        invoice: out.invoice,
        anchors: out.anchors,
    })
}
fn xml_of(bytes: &[u8]) -> Result<Cow<'_, [u8]>, EInvoiceError> {
    if bytes.starts_with(b"%PDF-") {
        pdf::embedded_xml(bytes).map(Cow::Owned)
    } else {
        Ok(Cow::Borrowed(bytes))
    }
}
fn format_of(root: &xml::Node) -> Result<Format, EInvoiceError> {
    match (root.ns.as_str(), root.name.as_str()) {
        (UBL_INVOICE, "Invoice") => Ok(Format::Ubl),
        (CII, "CrossIndustryInvoice") => Ok(Format::Cii),
        (UBL_CREDIT_NOTE, "CreditNote") => Err(EInvoiceError::Unsupported("UBL credit note")),
        _ => Err(EInvoiceError::NotAnEInvoice),
    }
}
#[derive(Default)]
struct Out {
    invoice: ExtractedInvoice,
    anchors: Vec<Anchor>,
    exponent: Option<u32>,
}
impl Out {
    fn mark(&mut self, field: &str, node: &xml::Node) {
        self.invoice.confidence.insert(field.to_owned(), 100);
        if !node.path.is_empty() && node.path.len() <= 500 {
            self.anchors.push(Anchor {
                field: field.to_owned(),
                location: Location::Xml {
                    path: node.path.clone(),
                },
            });
        }
    }
    fn text(&mut self, field: &str, node: Option<&xml::Node>) -> Option<String> {
        let n = node.filter(|n| !n.text.trim().is_empty())?;
        self.mark(field, n);
        Some(n.text.trim().to_owned())
    }
    fn date(&mut self, field: &str, node: Option<&xml::Node>) -> Option<String> {
        let n = node?;
        let d = xml::date(&n.text)?;
        self.mark(field, n);
        Some(d)
    }
    fn amount(&mut self, field: &str, node: Option<&xml::Node>) -> Option<i64> {
        let n = node?;
        let v = xml::decimal(&n.text, self.exponent?)?;
        self.mark(field, n);
        Some(v)
    }
    fn currency(&mut self, node: Option<&xml::Node>) {
        let code = self.text("currency", node);
        self.exponent = code.as_deref().and_then(exponent_of);
        self.invoice.currency = code;
    }
    fn line(
        &mut self,
        i: usize,
        name: Option<&xml::Node>,
        net: Option<&xml::Node>,
        rate: Option<&xml::Node>,
    ) {
        let desc = self.text(&format!("lines.{i}.description"), name);
        let net = self.amount(&format!("lines.{i}.net_minor"), net);
        let rate_bp = rate.and_then(|r| {
            xml::decimal(&r.text, 2).inspect(|_| self.mark(&format!("lines.{i}.vat_rate_bp"), r))
        });
        let vat = net.zip(rate_bp).map(|(n, r)| crate::checks::vat_for(n, r));
        let gross = net.zip(vat).and_then(|(n, v)| n.checked_add(v));
        self.invoice.lines.push(ExtractedLine {
            description: desc,
            net_minor: net,
            vat_rate_bp: rate_bp,
            vat_minor: vat,
            gross_minor: gross,
        });
    }
}
fn in_currency<'a>(
    mut amounts: impl Iterator<Item = &'a xml::Node>,
    currency: Option<&str>,
) -> Option<&'a xml::Node> {
    amounts.find(|a| a.attr("currencyID").is_none_or(|c| Some(c) == currency))
}
