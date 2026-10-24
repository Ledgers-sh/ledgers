//! E-invoice XML generation (`XRechnung` 3.0 UBL/CII, `Factur-X` EN 16931 CII).
//! Document text is data: all user input is XML-escaped.
//!
//! Generates outgoing invoices and credit notes in three profiles:
//! - `XRechnung` 3.0 UBL 2.1 with customization for German public procurement
//! - `XRechnung` 3.0 CII (UN/CEFACT `CrossIndustryInvoice` D16B)
//! - `Factur-X` / `ZUGFeRD` EN 16931 profile CII (XML only, no PDF container)
//!
//! All amounts use `ledgers_money` (minor units); integer-only arithmetic.
//! Percent rates are passed as strings after decimal validation (no float parsing).

use crate::model::InvoiceDocument;
use ledgers_ext::EInvoice;
use ledgers_money::Currency;
use std::fmt::Write;
use thiserror::Error;

/// E-invoice rendering profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    /// `XRechnung` 3.0 UBL 2.1 Invoice format.
    XRechnungUbl,
    /// `XRechnung` 3.0 CII (UN/CEFACT D16B) format.
    XRechnungCii,
    /// `Factur-X` / `ZUGFeRD` EN 16931 CII profile (XML only).
    FacturX,
}

/// E-invoice document: extends `InvoiceDocument` with structured parties and payment terms.
#[derive(Clone, Debug)]
pub struct EInvoiceDocument {
    /// Base invoice/credit note document.
    pub base: InvoiceDocument,
    /// Document type: "invoice" or `"credit_note"`.
    pub document_type: String,
    /// For credit notes: the preceding invoice number (BT-25).
    pub preceding_invoice_number: Option<String>,
    /// For credit notes: the preceding invoice date (optional).
    pub preceding_invoice_date: Option<String>,
    /// Seller: structured party with country code (ISO 3166-1 alpha-2).
    pub seller: PartyInfo,
    /// Buyer: structured party with country code.
    pub buyer: PartyInfo,
    /// Buyer reference (Leitweg-ID or customer order reference, required for B2G).
    pub buyer_reference: String,
    /// Seller contact (optional): name, phone, email.
    pub seller_contact: Option<ContactInfo>,
    /// Payment means: bank transfer (code 58), IBAN, optional BIC, account holder name.
    pub payment_means: Option<PaymentMeans>,
    /// Payment terms text (BT-20), optional.
    pub payment_terms_text: Option<String>,
}

/// Structured party information.
#[derive(Clone, Debug)]
pub struct PartyInfo {
    pub name: String,
    pub street: String,
    pub city: String,
    pub postal_code: String,
    /// ISO 3166-1 alpha-2 country code (e.g., "DE", "SE").
    pub country_code: String,
    pub vat_id: Option<String>,
    pub tax_number: Option<String>,
    /// Electronic address for BT-34/BT-49 (e-mail with schemeID "EM").
    pub email: Option<String>,
}

/// Seller contact: name, phone, email.
#[derive(Clone, Debug)]
pub struct ContactInfo {
    pub name: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
}

/// Payment means: bank transfer with IBAN.
#[derive(Clone, Debug)]
pub struct PaymentMeans {
    /// ISO 20022 code: "58" for SEPA credit transfer.
    pub code: String,
    pub iban: String,
    pub bic: Option<String>,
    pub account_name: Option<String>,
}

/// E-invoice generation error.
#[derive(Clone, Debug, Error)]
pub enum EInvoiceError {
    /// Missing required field (business term code and description).
    #[error("missing required field: {field} ({description})")]
    MissingField { field: String, description: String },
    /// Currency is not supported (exponent > 2).
    #[error("unsupported currency: {0} (max 2 decimal places)")]
    UnsupportedCurrency(String),
    /// Invalid unit code (not a known UN/ECE Rec 20/21 code).
    #[error(
        "unknown unit: {unit} (must be a valid UN/ECE code or one of: h, d, kg, m, km, l, pcs)"
    )]
    UnknownUnit { unit: String },
    /// Unsupported tax kind.
    #[error("unsupported tax kind: {kind} (expected: standard, intra-eu, reverse-charge)")]
    UnsupportedTaxKind { kind: String },
    /// Invalid decimal or arithmetic error.
    #[error("invalid decimal: {0}")]
    InvalidDecimal(String),
    /// Invalid quantity format.
    #[error("invalid quantity: {0}")]
    InvalidQuantity(String),
    /// Document structure error (e.g., credit note without preceding invoice number).
    #[error("invalid document structure: {0}")]
    InvalidDocument(String),
}

/// Convert i128 to i64, returning an error on overflow.
fn i128_to_i64(value: i128) -> Result<i64, EInvoiceError> {
    i64::try_from(value).map_err(|_| EInvoiceError::InvalidDecimal("amount overflow".to_string()))
}

/// Escape XML special characters.
fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Parse a decimal string into basis points (hundredths).
/// Valid formats: "19.00", "0.00", "7", etc.
/// Returns the integer representation (e.g., 1900 for "19.00").
/// Uses integer-only arithmetic (no float).
fn parse_decimal_to_bp(s: &str) -> Result<i32, EInvoiceError> {
    let trimmed = s.trim();

    // Split into integer and fractional parts
    let parts: Vec<&str> = trimmed.split('.').collect();
    if parts.is_empty() || parts.len() > 2 {
        return Err(EInvoiceError::InvalidDecimal(s.to_string()));
    }

    // Parse integer part
    let int_part: i32 = parts[0]
        .parse()
        .map_err(|_| EInvoiceError::InvalidDecimal(s.to_string()))?;

    // Parse fractional part if present
    let frac_part = if parts.len() == 2 {
        let frac_str = parts[1];
        if frac_str.is_empty() {
            0
        } else if frac_str.len() > 2 {
            // More than 2 decimal places; truncate to 2 places
            let trunc = &frac_str[..2];
            trunc
                .parse::<i32>()
                .map_err(|_| EInvoiceError::InvalidDecimal(s.to_string()))?
        } else if frac_str.len() == 2 {
            // Exactly 2 decimal places
            frac_str
                .parse::<i32>()
                .map_err(|_| EInvoiceError::InvalidDecimal(s.to_string()))?
        } else {
            // 1 decimal place: pad right with zero (e.g., "5" -> "50")
            let padded = format!("{frac_str}0");
            padded
                .parse::<i32>()
                .map_err(|_| EInvoiceError::InvalidDecimal(s.to_string()))?
        }
    } else {
        0
    };

    Ok(int_part * 100 + frac_part)
}

/// Validate quantity as a decimal string.
/// Validates format (digits and at most one decimal point) without float parsing.
/// Returns the string for XML output.
fn validate_quantity(s: &str) -> Result<String, EInvoiceError> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(EInvoiceError::InvalidQuantity("empty quantity".to_string()));
    }

    // Check format: optional sign, digits, optional decimal point, digits
    let mut dot_count = 0;
    for ch in trimmed.chars() {
        if ch == '.' {
            dot_count += 1;
            if dot_count > 1 {
                return Err(EInvoiceError::InvalidQuantity(trimmed.to_string()));
            }
        } else if ch == '-' || ch == '+' {
            // Allow sign only at start
            if !trimmed.starts_with(ch) {
                return Err(EInvoiceError::InvalidQuantity(trimmed.to_string()));
            }
        } else if !ch.is_ascii_digit() {
            return Err(EInvoiceError::InvalidQuantity(trimmed.to_string()));
        }
    }

    Ok(trimmed.to_string())
}

/// Map UN/ECE Rec 20/21 unit code.
fn map_unit_code(unit: &str) -> Result<String, EInvoiceError> {
    let trimmed = unit.trim();
    let lower = trimmed.to_lowercase();

    match lower.as_str() {
        // Time
        "h" | "hour" | "std" => Ok("HUR".to_string()),
        "d" | "day" | "tag" => Ok("DAY".to_string()),
        "month" | "monat" => Ok("MON".to_string()),
        // Weight
        "kg" | "kilogram" => Ok("KGM".to_string()),
        // Length
        "m" | "meter" => Ok("MTR".to_string()),
        "km" | "kilometer" => Ok("KMT".to_string()),
        // Volume
        "l" | "liter" => Ok("LTR".to_string()),
        // Piece
        "pcs" | "stk" | "piece" => Ok("C62".to_string()),
        // Percent
        "%" => Ok("P1".to_string()),
        _ => {
            // Accept already-valid 2-3 char uppercase UN/ECE codes
            if trimmed.len() >= 2
                && trimmed.len() <= 3
                && trimmed
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            {
                Ok(trimmed.to_string())
            } else {
                Err(EInvoiceError::UnknownUnit {
                    unit: unit.to_string(),
                })
            }
        }
    }
}

/// Validate VAT category from tax kind and return (`category_code`, `rate_bp`, `exemption_reason`).
fn validate_vat_category(
    kind: &str,
    rate_percent: &str,
    seller_vat_id: Option<&str>,
    buyer_vat_id: Option<&str>,
) -> Result<(String, i32, Option<String>), EInvoiceError> {
    let rate_bp = parse_decimal_to_bp(rate_percent)?;
    match kind {
        "standard" => {
            // Standard rate
            Ok(("S".to_string(), rate_bp, None))
        }
        "intra-eu" => {
            // Intra-EU supply: zero rate with exemption reason
            // BR-IC: requires seller VAT ID, buyer VAT ID, actual delivery date, deliver-to country
            if seller_vat_id.is_none() || buyer_vat_id.is_none() {
                return Err(EInvoiceError::InvalidDocument(
                    "intra-EU supply requires seller and buyer VAT IDs".to_string(),
                ));
            }
            Ok(("K".to_string(), 0, Some("VATEX-EU-IC".to_string())))
        }
        "reverse-charge" => {
            // Reverse charge: zero rate with exemption reason
            // BR-AE: requires seller and buyer VAT IDs
            if seller_vat_id.is_none() || buyer_vat_id.is_none() {
                return Err(EInvoiceError::InvalidDocument(
                    "reverse-charge supply requires seller and buyer VAT IDs".to_string(),
                ));
            }
            Ok(("AE".to_string(), 0, Some("VATEX-EU-AE".to_string())))
        }
        _ => Err(EInvoiceError::UnsupportedTaxKind {
            kind: kind.to_string(),
        }),
    }
}

/// Format minor units to canonical string using `ledgers_money`.
fn format_amount(minor: i64, currency: &str) -> Result<String, EInvoiceError> {
    let curr = Currency::from_code(currency)
        .map_err(|_| EInvoiceError::UnsupportedCurrency(currency.to_string()))?;

    // Check that currency has max 2 decimal places
    if curr.exponent() > 2 {
        return Err(EInvoiceError::UnsupportedCurrency(format!(
            "{} (max 2 decimals, has {})",
            currency,
            curr.exponent()
        )));
    }

    let amt = ledgers_money::Amount::new(i128::from(minor), curr);
    Ok(amt.to_canonical_string())
}

/// Validate all required fields for e-invoice generation.
fn validate_einvoice_document(doc: &EInvoiceDocument) -> Result<(), EInvoiceError> {
    if doc.base.invoice_number.is_empty() {
        return Err(EInvoiceError::MissingField {
            field: "invoice_number".to_string(),
            description: "invoice or credit note number (BT-1)".to_string(),
        });
    }

    if doc.seller.vat_id.is_none() && doc.seller.tax_number.is_none() {
        return Err(EInvoiceError::MissingField {
            field: "seller_vat_id or seller_tax_number".to_string(),
            description: "seller VAT ID or tax number (BT-31/BT-32)".to_string(),
        });
    }

    if doc.buyer_reference.is_empty() {
        return Err(EInvoiceError::MissingField {
            field: "buyer_reference".to_string(),
            description: "buyer reference (BT-10, Leitweg-ID or customer order ref)".to_string(),
        });
    }

    if doc.document_type == "credit_note" && doc.preceding_invoice_number.is_none() {
        return Err(EInvoiceError::MissingField {
            field: "preceding_invoice_number".to_string(),
            description: "preceding invoice number for credit note (BT-25)".to_string(),
        });
    }

    // Validate that all lines have tax rate and kind
    for (i, line) in doc.base.lines.iter().enumerate() {
        if line.tax_rate_percent.is_none() {
            return Err(EInvoiceError::MissingField {
                field: format!("lines[{i}].tax_rate_percent"),
                description: "tax rate for invoice line".to_string(),
            });
        }
        if line.tax_kind.is_none() {
            return Err(EInvoiceError::MissingField {
                field: format!("lines[{i}].tax_kind"),
                description: "tax kind for invoice line".to_string(),
            });
        }
    }

    Ok(())
}

/// Build UBL tax breakdown section XML.
fn build_ubl_tax_breakdown(
    doc: &EInvoiceDocument,
    currency: &str,
) -> Result<String, EInvoiceError> {
    let mut xml = String::new();
    for tb in &doc.base.tax_breakdown {
        let net = format_amount(tb.net_minor, currency)?;
        let tax = format_amount(tb.tax_minor, currency)?;
        let rate = &tb.tax_rate_percent;

        let (vat_category, _, _) = validate_vat_category(
            &tb.kind,
            rate,
            doc.seller.vat_id.as_deref(),
            doc.buyer.vat_id.as_deref(),
        )?;

        write!(
            xml,
            "\t\t<cac:TaxSubtotal>\n\t\t\t<cbc:TaxableAmount currencyID=\"{currency}\">{net}</cbc:TaxableAmount>\n\t\t\t<cbc:TaxAmount currencyID=\"{currency}\">{tax}</cbc:TaxAmount>\n\t\t\t<cac:TaxCategory>\n\t\t\t\t<cbc:ID>{vat_category}</cbc:ID>\n\t\t\t\t<cbc:Percent>{rate}</cbc:Percent>\n\t\t\t\t<cac:TaxScheme>\n\t\t\t\t\t<cbc:ID>VAT</cbc:ID>\n\t\t\t\t</cac:TaxScheme>\n\t\t\t</cac:TaxCategory>\n\t\t</cac:TaxSubtotal>\n"
        ).unwrap();
    }
    Ok(xml)
}

/// Build UBL invoice lines section XML.
fn build_ubl_invoice_lines(
    doc: &EInvoiceDocument,
    currency: &str,
) -> Result<String, EInvoiceError> {
    let mut xml = String::new();
    for line in &doc.base.lines {
        let unit = if let Some(ref u) = line.unit {
            map_unit_code(u)?
        } else {
            "C62".to_string()
        };

        let quantity = validate_quantity(&line.quantity)?;
        let net_amount = format_amount(line.net_minor, currency)?;
        let tax_rate = line.tax_rate_percent.as_ref().unwrap();
        let tax_kind = line.tax_kind.as_ref().unwrap();

        let (vat_category, _, _) = validate_vat_category(
            tax_kind,
            tax_rate,
            doc.seller.vat_id.as_deref(),
            doc.buyer.vat_id.as_deref(),
        )?;

        write!(
            xml,
            "\t\t<cac:InvoiceLine>\n\t\t\t<cbc:ID>{}</cbc:ID>\n\t\t\t<cbc:InvoicedQuantity unitCode=\"{}\">{}</cbc:InvoicedQuantity>\n\t\t\t<cbc:LineExtensionAmount currencyID=\"{}\">{}</cbc:LineExtensionAmount>\n\t\t\t<cac:Item>\n\t\t\t\t<cbc:Name>{}</cbc:Name>\n\t\t\t\t<cac:ClassifiedTaxCategory>\n\t\t\t\t\t<cbc:ID>{}</cbc:ID>\n\t\t\t\t\t<cbc:Percent>{}</cbc:Percent>\n\t\t\t\t\t<cac:TaxScheme>\n\t\t\t\t\t\t<cbc:ID>VAT</cbc:ID>\n\t\t\t\t\t</cac:TaxScheme>\n\t\t\t\t</cac:ClassifiedTaxCategory>\n\t\t\t</cac:Item>\n\t\t\t<cac:Price>\n\t\t\t\t<cbc:PriceAmount currencyID=\"{}\">{}</cbc:PriceAmount>\n\t\t\t\t<cbc:BaseQuantity unitCode=\"{}\">1</cbc:BaseQuantity>\n\t\t\t</cac:Price>\n\t\t</cac:InvoiceLine>\n",
            line.sequence,
            unit,
            quantity,
            currency,
            net_amount,
            escape_xml(&line.description),
            vat_category,
            tax_rate,
            currency,
            net_amount,
            unit,
        ).unwrap();
    }
    Ok(xml)
}

/// Assemble final UBL XML document.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
fn assemble_ubl_xml(
    doc: &EInvoiceDocument,
    _ns_prefix: &str,
    root_element: &str,
    invoice_type_code: &str,
    currency: &str,
    tax_breakdown_xml: &str,
    lines_xml: &str,
    billing_ref: &str,
    seller_vat: &str,
    buyer_vat: &str,
    net_total: &str,
    tax_total: &str,
    gross_total: &str,
) -> String {
    // Use default namespace for root element (conventional UBL approach)
    let root_ns = format!("urn:oasis:names:specification:ubl:schema:xsd:{root_element}-2");

    // Build seller electronic address (EndpointID as first child of Party)
    let seller_endpoint_xml = if let Some(ref email) = doc.seller.email {
        format!(
            "\t\t\t<cbc:EndpointID schemeID=\"EM\">{}</cbc:EndpointID>\n",
            escape_xml(email)
        )
    } else {
        String::new()
    };

    // Build seller legal entity
    let seller_legal_entity_xml = format!(
        "\t\t\t<cac:PartyLegalEntity>\n\t\t\t\t<cbc:RegistrationName>{}</cbc:RegistrationName>\n\t\t\t</cac:PartyLegalEntity>\n",
        escape_xml(&doc.seller.name)
    );

    // Build seller contact section
    let seller_contact_xml = if let Some(ref contact) = doc.seller_contact {
        use std::fmt::Write as FmtWrite;
        let mut contact_str = String::from("\t\t\t<cac:Contact>\n");
        if let Some(ref name) = contact.name {
            let _ = writeln!(
                contact_str,
                "\t\t\t\t<cbc:Name>{}</cbc:Name>",
                escape_xml(name)
            );
        }
        if let Some(ref phone) = contact.phone {
            let _ = writeln!(
                contact_str,
                "\t\t\t\t<cbc:Telephone>{}</cbc:Telephone>",
                escape_xml(phone)
            );
        }
        if let Some(ref email) = contact.email {
            let _ = writeln!(
                contact_str,
                "\t\t\t\t<cbc:ElectronicMail>{}</cbc:ElectronicMail>",
                escape_xml(email)
            );
        }
        contact_str.push_str("\t\t\t</cac:Contact>\n");
        contact_str
    } else {
        String::new()
    };

    // Build buyer electronic address
    let buyer_endpoint_xml = if let Some(ref email) = doc.buyer.email {
        format!(
            "\t\t\t<cbc:EndpointID schemeID=\"EM\">{}</cbc:EndpointID>\n",
            escape_xml(email)
        )
    } else {
        String::new()
    };

    // Build buyer legal entity
    let buyer_legal_entity_xml = format!(
        "\t\t\t<cac:PartyLegalEntity>\n\t\t\t\t<cbc:RegistrationName>{}</cbc:RegistrationName>\n\t\t\t</cac:PartyLegalEntity>\n",
        escape_xml(&doc.buyer.name)
    );

    // Build payment means section
    let payment_means_xml = if let Some(ref pm) = doc.payment_means {
        // Note: UBL invoices should not include InstructionID per UBL-CR-414
        let account_name_xml = if let Some(ref name) = pm.account_name {
            format!(
                "\t\t\t\t<cac:PayeeFinancialAccount>\n\t\t\t\t\t<cbc:ID>{}</cbc:ID>\n\t\t\t\t\t<cbc:Name>{}</cbc:Name>\n\t\t\t\t</cac:PayeeFinancialAccount>\n",
                escape_xml(&pm.iban),
                escape_xml(name)
            )
        } else {
            format!(
                "\t\t\t\t<cac:PayeeFinancialAccount>\n\t\t\t\t\t<cbc:ID>{}</cbc:ID>\n\t\t\t\t</cac:PayeeFinancialAccount>\n",
                escape_xml(&pm.iban)
            )
        };
        format!(
            "\t\t<cac:PaymentMeans>\n\t\t\t<cbc:PaymentMeansCode>{}</cbc:PaymentMeansCode>\n{}\t\t</cac:PaymentMeans>\n",
            escape_xml(&pm.code),
            account_name_xml
        )
    } else {
        String::new()
    };

    // Build payment terms section
    let payment_terms_xml = if let Some(ref terms) = doc.payment_terms_text {
        format!(
            "\t\t<cac:PaymentTerms>\n\t\t\t<cbc:Note>{}</cbc:Note>\n\t\t</cac:PaymentTerms>\n",
            escape_xml(terms)
        )
    } else {
        String::new()
    };

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<{} xmlns="{}" xmlns:cac="urn:oasis:names:specification:ubl:schema:xsd:CommonAggregateComponents-2" xmlns:cbc="urn:oasis:names:specification:ubl:schema:xsd:CommonBasicComponents-2" xmlns:ext="urn:oasis:names:specification:ubl:schema:xsd:CommonExtensionComponents-2">
	<cbc:CustomizationID>urn:cen.eu:en16931:2017#compliant#urn:xeinkauf.de:kosit:xrechnung_3.0</cbc:CustomizationID>
	<cbc:ProfileID>urn:fdc:peppol.eu:2017:poacc:billing:01:1.0</cbc:ProfileID>
	<cbc:ID>{}</cbc:ID>
	<cbc:IssueDate>{}</cbc:IssueDate>
	<cbc:DueDate>{}</cbc:DueDate>
	<cbc:InvoiceTypeCode>{}</cbc:InvoiceTypeCode>
	<cbc:DocumentCurrencyCode>{}</cbc:DocumentCurrencyCode>
	<cbc:BuyerReference>{}</cbc:BuyerReference>
{}	<cac:AccountingSupplierParty>
		<cac:Party>
			{}			<cac:PartyName>
				<cbc:Name>{}</cbc:Name>
			</cac:PartyName>
			<cac:PostalAddress>
				<cbc:StreetName>{}</cbc:StreetName>
				<cbc:CityName>{}</cbc:CityName>
				<cbc:PostalZone>{}</cbc:PostalZone>
				<cac:Country>
					<cbc:IdentificationCode>{}</cbc:IdentificationCode>
				</cac:Country>
			</cac:PostalAddress>
			<cac:PartyTaxScheme>
				<cbc:CompanyID>{}</cbc:CompanyID>
				<cac:TaxScheme>
					<cbc:ID>VAT</cbc:ID>
				</cac:TaxScheme>
			</cac:PartyTaxScheme>
			{}			{}		</cac:Party>
	</cac:AccountingSupplierParty>

	<cac:AccountingCustomerParty>
		<cac:Party>
			{}			<cac:PartyName>
				<cbc:Name>{}</cbc:Name>
			</cac:PartyName>
			<cac:PostalAddress>
				<cbc:StreetName>{}</cbc:StreetName>
				<cbc:CityName>{}</cbc:CityName>
				<cbc:PostalZone>{}</cbc:PostalZone>
				<cac:Country>
					<cbc:IdentificationCode>{}</cbc:IdentificationCode>
				</cac:Country>
			</cac:PostalAddress>
			<cac:PartyTaxScheme>
				<cbc:CompanyID>{}</cbc:CompanyID>
				<cac:TaxScheme>
					<cbc:ID>VAT</cbc:ID>
				</cac:TaxScheme>
			</cac:PartyTaxScheme>
			{}		</cac:Party>
	</cac:AccountingCustomerParty>
{}{}	<cac:TaxTotal>
		<cbc:TaxAmount currencyID="{}">{}</cbc:TaxAmount>
{}	</cac:TaxTotal>
	<cac:LegalMonetaryTotal>
		<cbc:LineExtensionAmount currencyID="{}">{}</cbc:LineExtensionAmount>
		<cbc:TaxExclusiveAmount currencyID="{}">{}</cbc:TaxExclusiveAmount>
		<cbc:TaxInclusiveAmount currencyID="{}">{}</cbc:TaxInclusiveAmount>
		<cbc:PayableAmount currencyID="{}">{}</cbc:PayableAmount>
	</cac:LegalMonetaryTotal>
{}
</{}>
"#,
        root_element,
        root_ns,
        escape_xml(&doc.base.invoice_number),
        escape_xml(&doc.base.issue_date),
        doc.base.due_date.as_deref().unwrap_or(&doc.base.issue_date),
        invoice_type_code,
        currency,
        escape_xml(&doc.buyer_reference),
        billing_ref,
        seller_endpoint_xml,
        escape_xml(&doc.seller.name),
        escape_xml(&doc.seller.street),
        escape_xml(&doc.seller.city),
        escape_xml(&doc.seller.postal_code),
        escape_xml(&doc.seller.country_code),
        escape_xml(seller_vat),
        seller_legal_entity_xml,
        seller_contact_xml,
        buyer_endpoint_xml,
        escape_xml(&doc.buyer.name),
        escape_xml(&doc.buyer.street),
        escape_xml(&doc.buyer.city),
        escape_xml(&doc.buyer.postal_code),
        escape_xml(&doc.buyer.country_code),
        escape_xml(buyer_vat),
        buyer_legal_entity_xml,
        payment_means_xml,
        payment_terms_xml,
        currency,
        tax_total,
        tax_breakdown_xml,
        currency,
        net_total,
        currency,
        net_total,
        currency,
        gross_total,
        currency,
        gross_total,
        lines_xml,
        root_element,
    )
}

/// Generate UBL 2.1 Invoice or `CreditNote` XML with correct element ordering.
fn render_ubl_invoice(doc: &EInvoiceDocument) -> Result<String, EInvoiceError> {
    validate_einvoice_document(doc)?;

    let currency = &doc.base.currency;
    let is_credit_note = doc.document_type == "credit_note";
    let invoice_type_code = if is_credit_note { "381" } else { "380" };
    let root_element = if is_credit_note {
        "CreditNote"
    } else {
        "Invoice"
    };
    let ns_prefix = if is_credit_note { "cn" } else { "inv" };

    let tax_breakdown_xml = build_ubl_tax_breakdown(doc, currency)?;
    let lines_xml = build_ubl_invoice_lines(doc, currency)?;

    let net_total = format_amount(doc.base.totals.net_minor, currency)?;
    let tax_total = format_amount(doc.base.totals.tax_minor, currency)?;
    let gross_total = format_amount(doc.base.totals.gross_minor, currency)?;

    let seller_vat = doc
        .seller
        .vat_id
        .as_deref()
        .or(doc.seller.tax_number.as_deref())
        .unwrap_or("UNKNOWN");
    let buyer_vat = doc
        .buyer
        .vat_id
        .as_deref()
        .or(doc.buyer.tax_number.as_deref())
        .unwrap_or("UNKNOWN");

    let billing_ref = if is_credit_note {
        format!(
            "\t<cac:BillingReference>\n\t\t<cac:InvoiceDocumentReference>\n\t\t\t<cbc:ID>{}</cbc:ID>\n\t\t</cac:InvoiceDocumentReference>\n\t</cac:BillingReference>\n",
            escape_xml(doc.preceding_invoice_number.as_ref().unwrap())
        )
    } else {
        String::new()
    };

    let xml = assemble_ubl_xml(
        doc,
        ns_prefix,
        root_element,
        invoice_type_code,
        currency,
        &tax_breakdown_xml,
        &lines_xml,
        &billing_ref,
        seller_vat,
        buyer_vat,
        &net_total,
        &tax_total,
        &gross_total,
    );

    Ok(xml)
}

/// Generate `XRechnung` 3.0 UBL Invoice XML from `EInvoiceDocument`.
fn render_xrechnung_ubl(doc: &EInvoiceDocument) -> Result<String, EInvoiceError> {
    // For now, use the same structure as UBL invoice
    render_ubl_invoice(doc)
}

/// Generate `XRechnung` 3.0 CII XML from `EInvoiceDocument`.
fn render_xrechnung_cii(doc: &EInvoiceDocument) -> Result<String, EInvoiceError> {
    render_cii_impl(
        doc,
        "urn:cen.eu:en16931:2017#compliant#urn:xeinkauf.de:kosit:xrechnung_3.0",
    )
}

/// Generate `Factur-X` / `ZUGFeRD` EN 16931 CII XML.
fn render_factur_x(doc: &EInvoiceDocument) -> Result<String, EInvoiceError> {
    render_cii_impl(doc, "urn:cen.eu:en16931:2017")
}

/// Build CII invoice lines section XML.
fn build_cii_invoice_lines(
    doc: &EInvoiceDocument,
    currency: &str,
) -> Result<String, EInvoiceError> {
    let mut xml = String::new();
    for line in &doc.base.lines {
        let quantity = validate_quantity(&line.quantity)?;
        let net_amount = format_amount(line.net_minor, currency)?;
        let tax_rate = line.tax_rate_percent.as_ref().unwrap();
        let tax_kind = line.tax_kind.as_ref().unwrap();

        let (_, _, _) = validate_vat_category(
            tax_kind,
            tax_rate,
            doc.seller.vat_id.as_deref(),
            doc.buyer.vat_id.as_deref(),
        )?;

        // Calculate tax for this line
        let line_tax_minor: i64 = doc
            .base
            .tax_breakdown
            .iter()
            .filter(|tb| tb.kind == *tax_kind && tb.tax_rate_percent == *tax_rate)
            .map(|tb| {
                if tb.net_minor > 0 {
                    let tax_calc = i128::from(line.net_minor) * i128::from(tb.tax_minor)
                        / i128::from(tb.net_minor);
                    i128_to_i64(tax_calc).unwrap_or(0)
                } else {
                    0
                }
            })
            .sum();

        let line_tax_amount = format_amount(line_tax_minor, currency)?;
        let gross_amount = format_amount(line.net_minor + line_tax_minor, currency)?;

        write!(
            xml,
            r#"			<ram:IncludedSupplyChainTradeLineItem>
				<ram:AssociatedDocumentLineDocument>
					<ram:LineID>{}</ram:LineID>
				</ram:AssociatedDocumentLineDocument>
				<ram:SpecifiedTradeProduct>
					<ram:Name>{}</ram:Name>
				</ram:SpecifiedTradeProduct>
				<ram:SpecifiedLineTradeAgreement>
					<ram:NetPriceProductTradePrice>
						<ram:ChargeAmount currencyID="{}">{}</ram:ChargeAmount>
						<ram:BasisQuantity unitCode="C62">{}</ram:BasisQuantity>
					</ram:NetPriceProductTradePrice>
				</ram:SpecifiedLineTradeAgreement>
				<ram:SpecifiedLineTradeDelivery>
					<ram:BilledQuantity unitCode="C62">{}</ram:BilledQuantity>
				</ram:SpecifiedLineTradeDelivery>
				<ram:SpecifiedLineTradeSettlement>
					<ram:ApplicableTradeTax>
						<ram:TypeCode>VAT</ram:TypeCode>
						<ram:RateApplicablePercent>{}</ram:RateApplicablePercent>
						<ram:CalculatedAmount currencyID="{}">{}</ram:CalculatedAmount>
						<ram:BasisAmount currencyID="{}">{}</ram:BasisAmount>
					</ram:ApplicableTradeTax>
					<ram:SpecifiedTradeSettlementLineMonetarySummation>
						<ram:LineTotalAmount currencyID="{}">{}</ram:LineTotalAmount>
					</ram:SpecifiedTradeSettlementLineMonetarySummation>
				</ram:SpecifiedLineTradeSettlement>
			</ram:IncludedSupplyChainTradeLineItem>
"#,
            line.sequence,
            escape_xml(&line.description),
            currency,
            net_amount,
            quantity,
            quantity,
            tax_rate,
            currency,
            line_tax_amount,
            currency,
            net_amount,
            currency,
            gross_amount,
        )
        .unwrap();
    }
    Ok(xml)
}

/// Shared CII renderer for `XRechnung` and `Factur-X` (parameterised by guideline ID).
fn render_cii_impl(doc: &EInvoiceDocument, guideline_id: &str) -> Result<String, EInvoiceError> {
    validate_einvoice_document(doc)?;

    let currency = &doc.base.currency;
    let is_credit_note = doc.document_type == "credit_note";

    let lines_xml = build_cii_invoice_lines(doc, currency)?;

    // Note: VAT breakdown is included in each line item via SpecifiedLineTradeSettlement

    let net_total = format_amount(doc.base.totals.net_minor, currency)?;
    let tax_total = format_amount(doc.base.totals.tax_minor, currency)?;
    let gross_total = format_amount(doc.base.totals.gross_minor, currency)?;

    let doc_type_code = if is_credit_note { "381" } else { "380" };

    let seller_vat_id = doc.seller.vat_id.as_deref().unwrap_or("");
    let buyer_vat_id = doc.buyer.vat_id.as_deref().unwrap_or("");

    let seller_vat_xml = if seller_vat_id.is_empty() {
        String::new()
    } else {
        format!(
            "\t\t\t\t\t<ram:VATID>{}</ram:VATID>\n",
            escape_xml(seller_vat_id)
        )
    };

    let buyer_vat_xml = if buyer_vat_id.is_empty() {
        String::new()
    } else {
        format!(
            "\t\t\t\t\t<ram:VATID>{}</ram:VATID>\n",
            escape_xml(buyer_vat_id)
        )
    };

    let buyer_reference_xml = if doc.buyer_reference.is_empty() {
        String::new()
    } else {
        format!(
            "\t\t\t<ram:BuyerReference>{}</ram:BuyerReference>\n",
            escape_xml(&doc.buyer_reference)
        )
    };

    let preceding_invoice_xml = if is_credit_note {
        let preceding_num = doc.preceding_invoice_number.as_ref().unwrap();
        format!(
            "\t\t\t<ram:InvoiceReferencedDocument>\n\t\t\t\t<ram:IssuerAssignedID>{}</ram:IssuerAssignedID>\n\t\t\t</ram:InvoiceReferencedDocument>\n",
            escape_xml(preceding_num)
        )
    } else {
        String::new()
    };

    let agreement =
        format_agreement_section(doc, &seller_vat_xml, &buyer_vat_xml, &buyer_reference_xml);
    let due_date_str = doc.base.due_date.as_deref().unwrap_or(&doc.base.issue_date);

    let xml = assemble_cii_xml(
        guideline_id,
        doc,
        doc_type_code,
        &preceding_invoice_xml,
        &agreement,
        &lines_xml,
        currency,
        due_date_str,
        &net_total,
        &tax_total,
        &gross_total,
    );

    Ok(xml)
}

/// Assemble final CII XML document.
#[allow(clippy::too_many_arguments)]
fn assemble_cii_xml(
    guideline_id: &str,
    doc: &EInvoiceDocument,
    doc_type_code: &str,
    preceding_invoice_xml: &str,
    agreement: &str,
    lines_xml: &str,
    currency: &str,
    due_date_str: &str,
    net_total: &str,
    tax_total: &str,
    gross_total: &str,
) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rsm:CrossIndustryInvoice
  xmlns:rsm="urn:un:unece:uncefact:data:standard:CrossIndustryInvoice:100"
  xmlns:qdt="urn:un:unece:uncefact:data:type:Quantity_1"
  xmlns:ram="urn:un:unece:uncefact:data:standard:ReusableAggregateBusinessInformationEntity:100"
  xmlns:udt="urn:un:unece:uncefact:data:standard:UnqualifiedDataType:100">
	<rsm:ExchangedDocumentContext>
		<ram:TestIndicator>
			<udt:Indicator>false</udt:Indicator>
		</ram:TestIndicator>
		<ram:GuidelineSpecifiedDocumentContextParameter>
			<ram:ID>{}</ram:ID>
		</ram:GuidelineSpecifiedDocumentContextParameter>
	</rsm:ExchangedDocumentContext>
	<rsm:ExchangedDocument>
		<ram:ID>{}</ram:ID>
		<ram:TypeCode>{}</ram:TypeCode>
		<ram:IssueDateTime>
			<udt:DateTimeString format="102">{}</udt:DateTimeString>
		</ram:IssueDateTime>
		{}	</rsm:ExchangedDocument>
	<rsm:SupplyChainTradeTransaction>
		{}{}		<ram:ApplicableHeaderTradeDelivery>
			<ram:ActualDeliveryDateTime>
				<udt:DateTimeString format="102">{}</udt:DateTimeString>
			</ram:ActualDeliveryDateTime>
		</ram:ApplicableHeaderTradeDelivery>
		<ram:ApplicableHeaderTradeSettlement>
			<ram:PaymentReference></ram:PaymentReference>
			<ram:TaxCurrencyCode>{}</ram:TaxCurrencyCode>
			<ram:InvoiceCurrencyCode>{}</ram:InvoiceCurrencyCode>
			<ram:SpecifiedTradePaymentTerms>
				<ram:DueDateDateTime>
					<udt:DateTimeString format="102">{}</udt:DateTimeString>
				</ram:DueDateDateTime>
			</ram:SpecifiedTradePaymentTerms>
			<ram:SpecifiedTradeSettlementHeaderMonetarySummation>
				<ram:DuePayableAmount currencyID="{}">{}</ram:DuePayableAmount>
				<ram:LineTotalBilledAmount currencyID="{}">{}</ram:LineTotalBilledAmount>
				<ram:ChargeTotalAmount currencyID="{}">{}</ram:ChargeTotalAmount>
				<ram:TaxBasisTotalAmount currencyID="{}">{}</ram:TaxBasisTotalAmount>
				<ram:TaxTotalAmount currencyID="{}">{}</ram:TaxTotalAmount>
				<ram:GrandTotalAmount currencyID="{}">{}</ram:GrandTotalAmount>
			</ram:SpecifiedTradeSettlementHeaderMonetarySummation>
		</ram:ApplicableHeaderTradeSettlement>
	</rsm:SupplyChainTradeTransaction>
</rsm:CrossIndustryInvoice>
"#,
        guideline_id,
        escape_xml(&doc.base.invoice_number),
        doc_type_code,
        escape_xml(&doc.base.issue_date),
        preceding_invoice_xml,
        agreement,
        lines_xml,
        escape_xml(&doc.base.supply_date),
        currency,
        currency,
        due_date_str,
        currency,
        gross_total,
        currency,
        net_total,
        currency,
        net_total,
        currency,
        net_total,
        currency,
        tax_total,
        currency,
        gross_total,
    )
}

/// Format the `ApplicableHeaderTradeAgreement` section for CII.
fn format_agreement_section(
    doc: &EInvoiceDocument,
    seller_vat_xml: &str,
    buyer_vat_xml: &str,
    buyer_reference_xml: &str,
) -> String {
    // Build seller electronic address
    let seller_email_xml = if let Some(ref email) = doc.seller.email {
        format!(
            "\t\t\t\t<ram:URIUniversalCommunication>\n\t\t\t\t\t<ram:URIID schemeID=\"EM\">{}</ram:URIID>\n\t\t\t\t</ram:URIUniversalCommunication>\n",
            escape_xml(email)
        )
    } else {
        String::new()
    };

    // Build seller legal organization
    let seller_legal_org_xml = format!(
        "\t\t\t\t<ram:SpecifiedLegalOrganization>\n\t\t\t\t\t<ram:TradingBusinessName>{}</ram:TradingBusinessName>\n\t\t\t\t</ram:SpecifiedLegalOrganization>\n",
        escape_xml(&doc.seller.name)
    );

    // Build buyer electronic address
    let buyer_email_xml = if let Some(ref email) = doc.buyer.email {
        format!(
            "\t\t\t\t<ram:URIUniversalCommunication>\n\t\t\t\t\t<ram:URIID schemeID=\"EM\">{}</ram:URIID>\n\t\t\t\t</ram:URIUniversalCommunication>\n",
            escape_xml(email)
        )
    } else {
        String::new()
    };

    // Build buyer legal organization
    let buyer_legal_org_xml = format!(
        "\t\t\t\t<ram:SpecifiedLegalOrganization>\n\t\t\t\t\t<ram:TradingBusinessName>{}</ram:TradingBusinessName>\n\t\t\t\t</ram:SpecifiedLegalOrganization>\n",
        escape_xml(&doc.buyer.name)
    );

    // Wrap seller VAT in SpecifiedTaxRegistration
    let seller_tax_reg_xml = if seller_vat_xml.is_empty() {
        String::new()
    } else {
        format!(
            "\t\t\t\t<ram:SpecifiedTaxRegistration>\n{seller_vat_xml}\t\t\t\t</ram:SpecifiedTaxRegistration>\n"
        )
    };

    // Wrap buyer VAT in SpecifiedTaxRegistration
    let buyer_tax_reg_xml = if buyer_vat_xml.is_empty() {
        String::new()
    } else {
        format!(
            "\t\t\t\t<ram:SpecifiedTaxRegistration>\n{buyer_vat_xml}\t\t\t\t</ram:SpecifiedTaxRegistration>\n"
        )
    };

    format!(
        r"		<ram:ApplicableHeaderTradeAgreement>
{}			<ram:SellerTradeParty>
				<ram:Name>{}</ram:Name>
				<ram:PostalTradeAddress>
					<ram:PostcodeCode>{}</ram:PostcodeCode>
					<ram:LineOne>{}</ram:LineOne>
					<ram:CityName>{}</ram:CityName>
					<ram:CountryID>{}</ram:CountryID>
				</ram:PostalTradeAddress>
{}{}{}			</ram:SellerTradeParty>
			<ram:BuyerTradeParty>
				<ram:Name>{}</ram:Name>
				<ram:PostalTradeAddress>
					<ram:PostcodeCode>{}</ram:PostcodeCode>
					<ram:LineOne>{}</ram:LineOne>
					<ram:CityName>{}</ram:CityName>
					<ram:CountryID>{}</ram:CountryID>
				</ram:PostalTradeAddress>
{}{}{}			</ram:BuyerTradeParty>
		</ram:ApplicableHeaderTradeAgreement>
",
        buyer_reference_xml,
        escape_xml(&doc.seller.name),
        escape_xml(&doc.seller.postal_code),
        escape_xml(&doc.seller.street),
        escape_xml(&doc.seller.city),
        escape_xml(&doc.seller.country_code),
        seller_email_xml,
        seller_legal_org_xml,
        seller_tax_reg_xml,
        escape_xml(&doc.buyer.name),
        escape_xml(&doc.buyer.postal_code),
        escape_xml(&doc.buyer.street),
        escape_xml(&doc.buyer.city),
        escape_xml(&doc.buyer.country_code),
        buyer_email_xml,
        buyer_legal_org_xml,
        buyer_tax_reg_xml,
    )
}

/// Generate an e-invoice in the specified profile and return it as `EInvoice`.
///
/// # Errors
/// Returns `EInvoiceError` if required fields are missing or data is invalid.
pub fn render_einvoice(
    doc: &EInvoiceDocument,
    profile: Profile,
) -> Result<EInvoice, EInvoiceError> {
    let (format_name, file_name, xml) = match profile {
        Profile::XRechnungUbl => {
            let xml = render_xrechnung_ubl(doc)?;
            (
                "xrechnung-ubl",
                format!("{}.xml", doc.base.invoice_number),
                xml,
            )
        }
        Profile::XRechnungCii => {
            let xml = render_xrechnung_cii(doc)?;
            (
                "xrechnung-cii",
                format!("{}.xml", doc.base.invoice_number),
                xml,
            )
        }
        Profile::FacturX => {
            let xml = render_factur_x(doc)?;
            ("factur-x", "factur-x.xml".to_string(), xml)
        }
    };

    Ok(EInvoice {
        format: format_name.to_string(),
        file_name,
        bytes: xml.into_bytes(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::InvoiceTotals;

    #[test]
    fn test_escape_xml() {
        let input = "<script>alert('xss')</script> & more";
        let expected = "&lt;script&gt;alert(&apos;xss&apos;)&lt;/script&gt; &amp; more";
        assert_eq!(escape_xml(input), expected);
    }

    #[test]
    fn test_parse_decimal_to_bp() {
        assert_eq!(parse_decimal_to_bp("19.00").unwrap(), 1900);
        assert_eq!(parse_decimal_to_bp("0.00").unwrap(), 0);
        assert_eq!(parse_decimal_to_bp("7.5").unwrap(), 750);
        assert_eq!(parse_decimal_to_bp("100").unwrap(), 10000);
        assert!(parse_decimal_to_bp("invalid").is_err());
    }

    #[test]
    fn test_validate_quantity() {
        assert_eq!(validate_quantity("10").unwrap(), "10");
        assert_eq!(validate_quantity("10.5").unwrap(), "10.5");
        assert!(validate_quantity("").is_err());
        assert!(validate_quantity("invalid").is_err());
    }

    #[test]
    fn test_map_unit_code() {
        assert_eq!(map_unit_code("h").unwrap(), "HUR");
        assert_eq!(map_unit_code("hour").unwrap(), "HUR");
        assert_eq!(map_unit_code("d").unwrap(), "DAY");
        assert_eq!(map_unit_code("kg").unwrap(), "KGM");
        assert_eq!(map_unit_code("m").unwrap(), "MTR");
        assert_eq!(map_unit_code("pcs").unwrap(), "C62");
        assert_eq!(map_unit_code("%").unwrap(), "P1");
        assert_eq!(map_unit_code("ABC").unwrap(), "ABC");
        assert!(map_unit_code("xyz").is_err());
    }

    #[test]
    fn test_validate_vat_category_standard() {
        let (cat, rate, reason) =
            validate_vat_category("standard", "19.00", Some("DE123"), Some("DE456")).unwrap();
        assert_eq!(cat, "S");
        assert_eq!(rate, 1900);
        assert_eq!(reason, None);
    }

    #[test]
    fn test_validate_vat_category_intra_eu() {
        let (cat, rate, reason) =
            validate_vat_category("intra-eu", "0.00", Some("DE123"), Some("SE456")).unwrap();
        assert_eq!(cat, "K");
        assert_eq!(rate, 0);
        assert_eq!(reason, Some("VATEX-EU-IC".to_string()));
    }

    #[test]
    fn test_validate_vat_category_intra_eu_missing_vat_id() {
        let result = validate_vat_category("intra-eu", "0.00", None, Some("SE456"));
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_vat_category_reverse_charge() {
        let (cat, rate, reason) =
            validate_vat_category("reverse-charge", "0.00", Some("DE123"), Some("DE456")).unwrap();
        assert_eq!(cat, "AE");
        assert_eq!(rate, 0);
        assert_eq!(reason, Some("VATEX-EU-AE".to_string()));
    }

    #[test]
    fn test_unsupported_tax_kind() {
        let result = validate_vat_category("unknown", "19.00", Some("DE123"), Some("DE456"));
        assert!(matches!(
            result,
            Err(EInvoiceError::UnsupportedTaxKind { .. })
        ));
    }

    #[test]
    fn test_validate_einvoice_document_missing_invoice_number() {
        let doc = EInvoiceDocument {
            base: InvoiceDocument {
                seller_name: "Seller".to_string(),
                seller_address: "Address".to_string(),
                seller_tax_number: Some("DE123".to_string()),
                seller_vat_id: None,
                buyer_name: "Buyer".to_string(),
                buyer_address: "Address".to_string(),
                buyer_vat_id: None,
                invoice_number: String::new(),
                issue_date: "2026-10-24".to_string(),
                supply_date: "2026-10-24".to_string(),
                lines: vec![],
                tax_breakdown: vec![],
                currency: "EUR".to_string(),
                totals: InvoiceTotals {
                    net_minor: 0,
                    tax_minor: 0,
                    gross_minor: 0,
                    currency: "EUR".to_string(),
                },
                due_date: None,
                note: None,
                intra_eu_note: None,
                reverse_charge_note: None,
            },
            document_type: "invoice".to_string(),
            preceding_invoice_number: None,
            preceding_invoice_date: None,
            seller: PartyInfo {
                name: "Seller".to_string(),
                street: "Street".to_string(),
                city: "City".to_string(),
                postal_code: "12345".to_string(),
                country_code: "DE".to_string(),
                vat_id: Some("DE123".to_string()),
                tax_number: None,
                email: None,
            },
            buyer: PartyInfo {
                name: "Buyer".to_string(),
                street: "Street".to_string(),
                city: "City".to_string(),
                postal_code: "12345".to_string(),
                country_code: "DE".to_string(),
                vat_id: None,
                tax_number: None,
                email: None,
            },
            buyer_reference: "REF123".to_string(),
            seller_contact: None,
            payment_means: None,
            payment_terms_text: None,
        };

        let result = validate_einvoice_document(&doc);
        assert!(matches!(result, Err(EInvoiceError::MissingField { .. })));
    }
}
