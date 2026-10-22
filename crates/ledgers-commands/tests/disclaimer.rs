//! Tests for disclaimer synchronization between code and documentation.

use ledgers_commands::NOT_TAX_ADVICE;

#[test]
fn disclaimer_doc_contains_constant() {
    let doc = include_str!("../../../docs/legal/DISCLAIMER.md");
    assert!(
        doc.contains(&format!("> {NOT_TAX_ADVICE}")),
        "DISCLAIMER.md must contain the blockquote: > {NOT_TAX_ADVICE}"
    );
}
