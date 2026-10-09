//! A search names its revision and either text or an extraction filter.

use okf_jawn_contract::error::ErrorCode;
use okf_jawn_contract::extraction::ExtractionFilter;
use okf_jawn_contract::identity::Revision;
use okf_jawn_core::search::SearchQuery;
use okf_jawn_core::storage::Page;

use check::{TestResult, err_of};

fn query(
    text: &str,
    extraction: Option<ExtractionFilter>,
) -> Result<SearchQuery, Box<dyn std::error::Error>> {
    Ok(SearchQuery {
        revision: Revision::try_from("a".repeat(40))?,
        text: text.to_owned(),
        folder: None,
        include_archived: false,
        extraction,
        page: Page {
            cursor: None,
            limit: 20,
        },
    })
}

#[test]
fn text_may_be_empty_only_with_an_extraction_filter() -> TestResult {
    query("quarterly revenue", None)?.check()?;
    query("quarterly revenue", Some(ExtractionFilter::Unprocessed))?.check()?;
    query("", Some(ExtractionFilter::Unprocessed))?.check()?;
    for blank in ["", "   "] {
        let refused = err_of(query(blank, None)?.check())?;
        assert_eq!(refused.code, ErrorCode::InvalidInput);
        assert_eq!(refused.field.as_deref(), Some("/text"));
    }
    Ok(())
}

#[path = "../../../tests/support/check.rs"]
mod check;
