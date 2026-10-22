//! File-type sniffing from the bytes. The upload's `Content-Type` and the
//! file name are claims; the magic numbers decide. Anything the app cannot
//! read is refused with the reason, so the person sees why.

/// The per-file size ceiling: 10 MiB, the buffered blob path's own limit.
pub const MAX_BYTES: usize = 10 * 1024 * 1024;

/// The most pages a PDF may have. Longer statements are refused: they are
/// not single invoices, and extraction cost grows with pages.
pub const MAX_PDF_PAGES: usize = 50;

/// A file type the document store accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `application/pdf`.
    Pdf,
    /// `image/png`.
    Png,
    /// `image/jpeg`.
    Jpeg,
}

impl Kind {
    /// The MIME type stored with the document.
    #[must_use]
    pub const fn mime(self) -> &'static str {
        match self {
            Kind::Pdf => "application/pdf",
            Kind::Png => "image/png",
            Kind::Jpeg => "image/jpeg",
        }
    }
}

/// Why a file was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// The file is empty.
    #[error("the file is empty")]
    Empty,
    /// The file is larger than [`MAX_BYTES`].
    #[error("the file is {size} bytes; the limit is {MAX_BYTES} bytes")]
    TooLarge {
        /// The size of the file.
        size: usize,
    },
    /// The bytes are not a PDF, PNG or JPEG.
    #[error("only PDF, PNG and JPEG files can be stored; this file is not one of them")]
    UnsupportedType,
    /// HEIC photos must be converted to JPEG on the device first.
    #[error("HEIC photos must be converted to JPEG before upload")]
    Heic,
    /// Encrypted PDFs cannot be read for extraction.
    #[error("encrypted PDFs cannot be read; export an unlocked copy")]
    EncryptedPdf,
    /// The PDF has more pages than [`MAX_PDF_PAGES`].
    #[error("the PDF has {pages} pages; the limit is {MAX_PDF_PAGES}")]
    TooManyPages {
        /// The estimated page count.
        pages: usize,
    },
    /// The declared `Content-Type` names a different type than the bytes.
    #[error("the declared content type {declared} does not match the file")]
    TypeMismatch {
        /// The declared MIME type.
        declared: String,
    },
}

/// Sniffs the type of `bytes` and applies the size, page and encryption rules.
///
/// # Errors
/// Returns the [`Refusal`] naming why the file cannot be stored.
pub fn sniff(bytes: &[u8]) -> Result<Kind, Refusal> {
    if bytes.is_empty() {
        return Err(Refusal::Empty);
    }
    if bytes.len() > MAX_BYTES {
        return Err(Refusal::TooLarge { size: bytes.len() });
    }
    if is_heic(bytes) {
        return Err(Refusal::Heic);
    }
    if bytes.starts_with(b"%PDF-") {
        if contains(bytes, b"/Encrypt") {
            return Err(Refusal::EncryptedPdf);
        }
        let pages = pdf_page_estimate(bytes);
        if pages > MAX_PDF_PAGES {
            return Err(Refusal::TooManyPages { pages });
        }
        return Ok(Kind::Pdf);
    }
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Ok(Kind::Png);
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Ok(Kind::Jpeg);
    }
    Err(Refusal::UnsupportedType)
}

/// Checks the declared `Content-Type` against the sniffed kind. A missing
/// header is accepted; a header naming another type is refused.
///
/// # Errors
/// [`Refusal::TypeMismatch`] when the declared type is not the sniffed one.
pub fn check_declared(declared: Option<&str>, kind: Kind) -> Result<(), Refusal> {
    match declared {
        None => Ok(()),
        Some(value) => {
            let base = value.split(';').next().unwrap_or("").trim();
            let base = if base == "image/jpg" {
                "image/jpeg"
            } else {
                base
            };
            if base.eq_ignore_ascii_case(kind.mime()) {
                Ok(())
            } else {
                Err(Refusal::TypeMismatch {
                    declared: base.to_owned(),
                })
            }
        }
    }
}

fn is_heic(bytes: &[u8]) -> bool {
    bytes.len() >= 12
        && &bytes[4..8] == b"ftyp"
        && matches!(
            &bytes[8..12],
            b"heic" | b"heix" | b"heim" | b"heis" | b"hevc" | b"mif1" | b"msf1"
        )
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Estimates the page count from the page objects (`/Type /Page` that is not
/// `/Pages`). Object streams hide some pages from this scan, so it is an
/// estimate used only for the cap; extraction counts the real pages.
fn pdf_page_estimate(bytes: &[u8]) -> usize {
    let mut count = 0usize;
    let mut i = 0usize;
    while i + 10 <= bytes.len() {
        if bytes[i..].starts_with(b"/Type") {
            let rest = &bytes[i + 5..bytes.len().min(i + 16)];
            let trimmed: Vec<u8> = rest
                .iter()
                .copied()
                .filter(|b| !b.is_ascii_whitespace())
                .collect();
            if trimmed.starts_with(b"/Page") && !trimmed.starts_with(b"/Pages") {
                count += 1;
            }
        }
        i += 1;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pdf(pages: usize) -> Vec<u8> {
        let mut bytes = b"%PDF-1.7\n".to_vec();
        bytes.extend_from_slice(b"1 0 obj << /Type /Pages /Count 1 >> endobj\n");
        for _ in 0..pages {
            bytes.extend_from_slice(b"2 0 obj << /Type /Page /Parent 1 0 R >> endobj\n");
        }
        bytes
    }

    #[test]
    fn sniffs_by_magic_number_not_by_claim() {
        assert_eq!(sniff(&pdf(1)), Ok(Kind::Pdf));
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\nrest"), Ok(Kind::Png));
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10]), Ok(Kind::Jpeg));
    }

    #[test]
    fn refuses_an_executable_renamed_to_pdf() {
        // The bytes are an ELF binary; no declared type or file name can make
        // them a PDF.
        assert_eq!(
            sniff(b"\x7fELF\x02\x01\x01\x00"),
            Err(Refusal::UnsupportedType)
        );
    }

    #[test]
    fn refuses_a_declared_type_that_does_not_match_the_bytes() {
        let kind = sniff(&pdf(1)).expect("pdf");
        assert_eq!(check_declared(Some("application/pdf"), kind), Ok(()));
        assert_eq!(check_declared(None, kind), Ok(()));
        assert_eq!(
            check_declared(Some("image/png"), kind),
            Err(Refusal::TypeMismatch {
                declared: "image/png".to_owned()
            })
        );
    }

    #[test]
    fn refuses_empty_oversize_heic_and_encrypted() {
        assert_eq!(sniff(&[]), Err(Refusal::Empty));
        let big = vec![b'%'; MAX_BYTES + 1];
        assert_eq!(
            sniff(&big),
            Err(Refusal::TooLarge {
                size: MAX_BYTES + 1
            })
        );
        let mut heic = vec![0, 0, 0, 0x18];
        heic.extend_from_slice(b"ftypheic");
        assert_eq!(sniff(&heic), Err(Refusal::Heic));
        let mut encrypted = b"%PDF-1.6\n".to_vec();
        encrypted.extend_from_slice(b"trailer << /Encrypt 5 0 R >>");
        assert_eq!(sniff(&encrypted), Err(Refusal::EncryptedPdf));
    }

    #[test]
    fn refuses_pdfs_over_the_page_cap() {
        assert_eq!(sniff(&pdf(MAX_PDF_PAGES)), Ok(Kind::Pdf));
        assert_eq!(
            sniff(&pdf(MAX_PDF_PAGES + 1)),
            Err(Refusal::TooManyPages {
                pages: MAX_PDF_PAGES + 1
            })
        );
    }
}
