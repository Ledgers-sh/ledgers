//! The PDF text layer, read with `lopdf` (pure Rust, no default features).
//! A PDF without a text layer (a scan) yields `None`; the model reads those.

/// Reads the text of every page of a PDF.
///
/// # Errors
/// A message when the file cannot be parsed (corrupt or unsupported PDF).
pub fn text_layer(bytes: &[u8]) -> Result<Option<String>, String> {
    let document = lopdf::Document::load_mem(bytes).map_err(|err| err.to_string())?;
    let pages: Vec<u32> = document.get_pages().keys().copied().collect();
    if pages.is_empty() {
        return Ok(None);
    }
    let text = document
        .extract_text(&pages)
        .map_err(|err| err.to_string())?;
    if text.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(text))
    }
}
