//! Serialization of public data types with the `serde` feature.
#![cfg(feature = "serde")]

use legible::{ContentHint, ContentMetrics, Extractor, Metadata, ParseBudget};

#[test]
fn public_data_types_round_trip_through_json() {
    let page = Extractor::builder()
        .diagnostics(true)
        .metadata_diagnostics(true)
        .build()
        .extract(
            r#"<html lang="en"><head><title>Harbour notes</title><meta name="author" content="Ada Lovelace"></head><body><main><h1>Harbour notes</h1><p>The harbour opened at dawn and the first ferry left on time for the island.</p></main></body></html>"#,
            None,
        )
        .unwrap();

    let metadata = serde_json::to_value(page.metadata()).unwrap();
    assert_eq!(metadata["title"], "Harbour notes");
    assert_eq!(metadata["authors"][0], "Ada Lovelace");
    assert!(metadata.get("has_source_author").is_none(), "{metadata}");
    let decoded: Metadata = serde_json::from_value(metadata).unwrap();
    assert_eq!(decoded.title.as_deref(), Some("Harbour notes"));

    let metrics = page.content().metrics();
    let decoded: ContentMetrics =
        serde_json::from_value(serde_json::to_value(metrics).unwrap()).unwrap();
    assert_eq!(decoded, metrics);

    let diagnostics = serde_json::to_value(page.diagnostics().unwrap()).unwrap();
    assert!(
        diagnostics["attempts"]
            .as_array()
            .is_some_and(|attempts| !attempts.is_empty())
    );
    assert_eq!(diagnostics["selected_strategy"], "normal");
    let provenance = serde_json::to_value(page.metadata_diagnostics().unwrap()).unwrap();
    assert_eq!(provenance["title"]["selected"]["value"], "Harbour notes");

    let budget = ParseBudget::untrusted();
    let decoded: ParseBudget =
        serde_json::from_value(serde_json::to_value(&budget).unwrap()).unwrap();
    assert_eq!(decoded, budget);

    let hint: ContentHint = serde_json::from_str(r#"{"class":"article-body"}"#).unwrap();
    assert_eq!(hint, ContentHint::Class("article-body".into()));
}
