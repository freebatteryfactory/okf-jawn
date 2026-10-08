//! A source card's file name is derived from the original name, keeping its extension visible.

use okf_jawn_contract::conventions::source_card_name;

#[test]
fn source_card_name_joins_the_extension() {
    for (original, card) in [
        ("report.pdf", "report-pdf.md"),
        ("v1.2.notes.txt", "v1.2.notes-txt.md"),
        ("README", "README.md"),
        (".env", ".env.md"),
        ("Q3 Results.XLSX", "Q3 Results-XLSX.md"),
    ] {
        assert_eq!(source_card_name(original), card, "{original}");
    }
}
