use legible::{Options, parse};

#[test]
fn short_article_fallback_does_not_import_synthetic_html() {
    let article = parse(
        "<html><body><article><p>This is a short article.</p></article></body></html>",
        None,
        None,
    )
    .unwrap();

    assert!(article.html().contains("id=\"readability-page-1\""));
    assert!(!article.html().contains("<html>"));
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

    let article = parse(
        html,
        Some("https://example.com/news/page.html"),
        Some(Options::new().char_threshold(0)),
    )
    .unwrap();

    assert_eq!(article.lang.as_deref(), Some("fr"));
    let html = article.html();
    let text = article.text();
    assert_eq!(html, article.html());
    assert_eq!(text, article.text());
    assert_eq!(text.chars().count(), article.length);
    let clone = article.clone();
    assert_eq!(html, clone.html());
    assert_eq!(text, clone.text());
    assert!(text.contains("A relative link"));
    assert_eq!(
        article.published_time.as_deref(),
        Some("2024-06-01T12:30:00Z")
    );
    assert_eq!(
        article.html(),
        r#"<div id="readability-page-1" class="page"><article><p>
        <a href="https://example.com/story?x=1&amp;y=2">A relative link</a>
        <img src="https://example.com/news/images/photo.jpg" srcset="https://example.com/news/small.jpg 1x, https://example.com/large.jpg 2x" alt="Photo">
        This paragraph has enough useful article text for deterministic extraction.
        </p></article></div>"#
    );
}
