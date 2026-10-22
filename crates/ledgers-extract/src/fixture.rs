//! Invented documents for tests: a valid one-page PDF with a text layer, and
//! the magic bytes of the image types. Nothing here is a real invoice.

use std::fmt::Write;

/// A valid one-page PDF (with a cross-reference table) whose text layer holds
/// `lines`, one per text line. Parentheses and backslashes are escaped.
#[must_use]
pub fn pdf_with_text(lines: &[&str]) -> Vec<u8> {
    // One text object per line, placed absolutely: that is how producers lay
    // out invoices, and it keeps the line breaks in the extracted text layer.
    let mut content = String::new();
    for (index, line) in lines.iter().enumerate() {
        let escaped = line
            .replace('\\', "\\\\")
            .replace('(', "\\(")
            .replace(')', "\\)");
        let y = 800 - 16 * i64::try_from(index).unwrap_or(0);
        let _ = writeln!(content, "BT /F1 12 Tf 1 0 0 1 50 {y} Tm ({escaped}) Tj ET");
    }
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R \
         /Resources << /Font << /F1 5 0 R >> >> >>"
            .to_owned(),
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
    ];
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (index, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", index + 1).as_bytes());
    }
    let xref_at = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// The magic bytes of a PNG, followed by a short invented body.
#[must_use]
pub fn png_bytes() -> Vec<u8> {
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    bytes.extend_from_slice(b"invented image body");
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fixture_pdf_reads_back_its_text_layer() {
        let pdf = pdf_with_text(&["Invoice No: RE-1", "Total 119,00 EUR"]);
        let text = crate::pdf::text_layer(&pdf)
            .expect("parses")
            .expect("has text");
        assert!(text.contains("RE-1"));
        assert!(text.contains("119,00"));
    }
}
