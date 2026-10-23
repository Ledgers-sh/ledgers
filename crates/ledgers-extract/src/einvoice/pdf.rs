//! The XML a ZUGFeRD/Factur-X PDF carries as an embedded file.
use super::EInvoiceError;
use std::io::Read;

/// Maximum PDF file size: 10 MiB (matching the document store upload limit).
const MAX_PDF_BYTES: usize = 10 * 1024 * 1024;
/// Maximum decompressed object stream size: 16 MiB (prevents ZIP bombs in PDF object streams).
const MAX_DECOMPRESSED: usize = 16 * 1024 * 1024;

pub fn embedded_xml(bytes: &[u8]) -> Result<Vec<u8>, EInvoiceError> {
    if bytes.len() > MAX_PDF_BYTES {
        return Err(EInvoiceError::TooLarge);
    }
    let opts = lopdf::LoadOptions::with_max_decompressed_size(MAX_DECOMPRESSED);
    let doc = lopdf::Document::load_mem_with_options(bytes, opts)
        .map_err(|_| EInvoiceError::NoEmbeddedInvoice)?;
    for object in doc.objects.values() {
        let Ok(stream) = object.as_stream() else {
            continue;
        };
        let dict = &stream.dict;
        if !matches!(
            dict.get(b"Type").ok().and_then(|t| t.as_name().ok()),
            Some(b"EmbeddedFile")
        ) {
            continue;
        }
        let xml = match dict.get(b"Filter") {
            Ok(filter) => {
                if let Ok(name) = filter.as_name() {
                    if name == b"FlateDecode" {
                        inflate(&stream.content)?
                    } else {
                        continue;
                    }
                } else if let Ok(arr) = filter.as_array() {
                    if arr.len() == 1 {
                        if let Ok(name) = arr[0].as_name() {
                            if name == b"FlateDecode" {
                                inflate(&stream.content)?
                            } else {
                                continue;
                            }
                        } else {
                            continue;
                        }
                    } else {
                        continue;
                    }
                } else {
                    continue;
                }
            }
            Err(_) => stream.content.clone(),
        };
        if super::xml::Doc::parse(&xml).is_ok_and(|d| super::format_of(d.root()).is_ok()) {
            return Ok(xml);
        }
    }
    Err(EInvoiceError::NoEmbeddedInvoice)
}

/// zlib with the XML cap applied while inflating.
fn inflate(data: &[u8]) -> Result<Vec<u8>, EInvoiceError> {
    let mut out = Vec::new();
    let decoder = flate2::read::ZlibDecoder::new(data);
    let mut limited = decoder.take(u64::try_from(super::MAX_XML_BYTES).unwrap_or(u64::MAX) + 1);
    limited
        .read_to_end(&mut out)
        .map_err(|_| EInvoiceError::Malformed("embedded file is not zlib".into()))?;
    if out.len() > super::MAX_XML_BYTES {
        Err(EInvoiceError::TooLarge)
    } else {
        Ok(out)
    }
}
