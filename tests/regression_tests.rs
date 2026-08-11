use legible::extract;

#[test]
fn short_content_fallback_does_not_import_synthetic_html() {
    let page = extract(
        "<html><body><article><p>This is a short article.</p></article></body></html>",
        None,
    )
    .unwrap();

    assert!(page.html().contains("id=\"legible-content\""));
    assert!(!page.html().contains("<html>"));
}

#[test]
fn exact_output_preserves_metadata_and_rewrites_urls() {
    let html = r#"<!doctype html><html lang="fr"><head>
        <title>Exact article</title>
        <meta property="article:published_time" content="2024-06-01T12:30:00Z">
        </head><body><article><p>
        <a href="../story?x=1&amp;y=2">A relative link</a>
        <img src="images/photo.jpg" srcset="small.jpg 1x, /large.jpg 2x" alt="Photo">
        This paragraph has enough useful article text for deterministic extraction.
        </p></article></body></html>"#;

    let page = extract(html, Some("https://example.com/news/page.html")).unwrap();

    assert_eq!(page.metadata().language.as_deref(), Some("fr"));
    assert_eq!(
        page.metadata().published_time.as_deref(),
        Some("2024-06-01T12:30:00Z")
    );
    assert_eq!(
        page.html(),
        r#"<div id="legible-content" class="page"><article><p>
        <a href="https://example.com/story?x=1&amp;y=2">A relative link</a>
        <img src="https://example.com/large.jpg" srcset="https://example.com/news/small.jpg 1x, https://example.com/large.jpg 2x" alt="Photo">
        This paragraph has enough useful article text for deterministic extraction.
        </p></article></div>"#
    );
}
