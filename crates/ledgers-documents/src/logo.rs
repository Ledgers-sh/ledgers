//! Logos for companies, groups and business units. A logo is an ordinary
//! stored document that is a PNG or a JPEG no larger than
//! [`MAX_LOGO_BYTES`]. SVG is never stored: an SVG can carry script, so the
//! Factory Zero marks are rendered to PNG before they are uploaded.

/// The largest logo the app stores: 2 MiB. Documents in general may be 10 MiB;
/// a logo is drawn at 20 to 64 pixels, so the limit is far above what it needs.
pub const MAX_LOGO_BYTES: i64 = 2 * 1024 * 1024;

/// Why a document cannot be a logo.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LogoRefusal {
    /// The document is not a PNG or a JPEG.
    #[error("a logo must be a PNG or JPEG image")]
    NotImage,
    /// The document is larger than [`MAX_LOGO_BYTES`].
    #[error("a logo may be at most {MAX_LOGO_BYTES} bytes; this one is {size} bytes")]
    TooLarge {
        /// The size of the document.
        size: i64,
    },
}

/// Checks a stored document's type and size against the logo rule.
///
/// # Errors
/// [`LogoRefusal::NotImage`] for any type but `image/png` or `image/jpeg`, and
/// [`LogoRefusal::TooLarge`] above [`MAX_LOGO_BYTES`].
pub fn check_logo(mime: &str, size_bytes: i64) -> Result<(), LogoRefusal> {
    if mime != "image/png" && mime != "image/jpeg" {
        return Err(LogoRefusal::NotImage);
    }
    if size_bytes > MAX_LOGO_BYTES {
        return Err(LogoRefusal::TooLarge { size: size_bytes });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_and_jpeg_within_the_limit_are_logos() {
        assert_eq!(check_logo("image/png", 1024), Ok(()));
        assert_eq!(check_logo("image/jpeg", MAX_LOGO_BYTES), Ok(()));
    }

    #[test]
    fn other_types_are_refused_including_svg() {
        for mime in [
            "image/svg+xml",
            "application/pdf",
            "text/csv",
            "application/xml",
            "",
        ] {
            assert_eq!(check_logo(mime, 10), Err(LogoRefusal::NotImage), "{mime}");
        }
    }

    #[test]
    fn a_document_one_byte_over_the_limit_is_refused() {
        assert_eq!(
            check_logo("image/png", MAX_LOGO_BYTES + 1),
            Err(LogoRefusal::TooLarge {
                size: MAX_LOGO_BYTES + 1
            })
        );
    }
}
