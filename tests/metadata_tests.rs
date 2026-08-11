use legible::{Extractor, extract};

const CONTENT: &str = "This page has enough meaningful content for extraction. It explains the subject with clear details and useful context for every reader.";

#[test]
fn resolves_rich_metadata_through_the_public_api() {
    let html = format!(
        r#"<html lang="en_US" dir="ltr"><head>
        <title>Building caches | Example</title>
        <meta property="og:title" content="Building caches">
        <meta property="og:description" content="A practical guide &amp; reference.">
        <meta property="og:site_name" content="Example">
        <meta property="og:image" content="/images/hero.jpg">
        <meta property="article:modified_time" content="2025-02-04">
        <meta property="article:section" content="Engineering">
        <meta property="article:tag" content="Rust">
        <meta name="keywords" content="rust, Caches">
        <link rel="canonical" href="/guides/caches">
        <link rel="icon" href="assets/icon.png">
        <script type="application/ld+json">{{
          "@context":"https://schema.org", "@type":"TechArticle",
          "headline":"Building caches", "author":[{{"name":"Ada"}},{{"name":"Grace"}}],
          "datePublished":"2025-02-03"
        }}</script></head><body><article><h1>Building caches</h1><p>{CONTENT}</p></article></body></html>"#
    );

    let page = extract(&html, Some("https://example.com/docs/page")).unwrap();
    let metadata = page.metadata();

    assert_eq!(metadata.title.as_deref(), Some("Building caches"));
    assert_eq!(
        metadata.description.as_deref(),
        Some("A practical guide & reference.")
    );
    assert_eq!(metadata.authors, ["Ada", "Grace"]);
    assert_eq!(metadata.site_name.as_deref(), Some("Example"));
    assert_eq!(
        metadata.canonical_url.as_deref(),
        Some("https://example.com/guides/caches")
    );
    assert_eq!(
        metadata.image.as_deref(),
        Some("https://example.com/images/hero.jpg")
    );
    assert_eq!(
        metadata.favicon.as_deref(),
        Some("https://example.com/docs/assets/icon.png")
    );
    assert_eq!(metadata.published_time.as_deref(), Some("2025-02-03"));
    assert_eq!(metadata.modified_time.as_deref(), Some("2025-02-04"));
    assert_eq!(metadata.language.as_deref(), Some("en-US"));
    assert_eq!(metadata.direction.as_deref(), Some("ltr"));
    assert_eq!(metadata.section.as_deref(), Some("Engineering"));
    assert_eq!(metadata.tags, ["Rust", "Caches"]);
}

#[test]
fn keeps_source_url_fallbacks_separate_from_the_base_element() {
    let with_relative_canonical = format!(
        r#"<html><head><base href="https://cdn.example.net/assets/"><link rel="canonical" href="page"></head><body><main><p>{CONTENT}</p></main></body></html>"#
    );
    let page = extract(
        &with_relative_canonical,
        Some("https://www.example.com/original"),
    )
    .unwrap();
    assert_eq!(
        page.metadata().canonical_url.as_deref(),
        Some("https://cdn.example.net/assets/page")
    );
    assert_eq!(page.metadata().site_name.as_deref(), Some("example.com"));

    let absolute_base_without_source = format!(
        r#"<html><head><base href="https://cdn.example.net/assets/"><link rel="canonical" href="page"></head><body><main><p>{CONTENT}</p></main></body></html>"#
    );
    let page = extract(&absolute_base_without_source, None).unwrap();
    assert_eq!(
        page.metadata().canonical_url.as_deref(),
        Some("https://cdn.example.net/assets/page")
    );

    let without_canonical = format!(
        r#"<html><head><base href="https://cdn.example.net/assets/"></head><body><main><p>{CONTENT}</p></main></body></html>"#
    );
    let page = extract(&without_canonical, Some("https://www.example.com/original")).unwrap();
    assert_eq!(
        page.metadata().canonical_url.as_deref(),
        Some("https://www.example.com/original")
    );
    assert_eq!(page.metadata().site_name.as_deref(), Some("example.com"));
}

#[test]
fn uses_dom_byline_fallback_and_can_disable_structured_data() {
    let html = format!(
        r#"<html><head><title>Page</title><script type="application/ld+json">{{
        "@context":"https://schema.org", "@type":"Article", "author":{{"name":"Schema Author"}}
        }}</script></head><body><article><h1>Page</h1><div class="byline">By DOM Author</div><p>{CONTENT}</p></article></body></html>"#
    );
    let page = Extractor::builder()
        .structured_data(false)
        .build()
        .extract(&html, None)
        .unwrap();

    assert_eq!(page.metadata().authors, ["DOM Author"]);
}
