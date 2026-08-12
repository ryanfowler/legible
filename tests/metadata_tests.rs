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
fn filters_template_metadata_before_applying_source_precedence() {
    let html = format!(
        r#"<html lang="undefined"><head>
        <title>Default Title</title>
        <meta property="og:title" content="Practical metadata">
        <meta property="og:description" content="Default description">
        <meta name="twitter:description" content="A useful page summary.">
        <meta property="og:site_name" content="N/A">
        <meta property="article:section" content="N/A">
        <meta name="author" content="Your Name">
        <meta name="citation_author" content="Ada Lovelace">
        <script type="application/ld+json">{{
          "@context":"https://schema.org", "@type":"Article",
          "headline":"Default Title", "author":{{"name":"Unknown"}}
        }}</script></head><body><main><h1>Practical metadata</h1><p>{CONTENT}</p></main></body></html>"#
    );

    let page = Extractor::builder()
        .metadata_diagnostics(true)
        .build()
        .extract(&html, Some("https://example.com/article"))
        .unwrap();
    let metadata = page.metadata();
    let diagnostics = page.metadata_diagnostics().unwrap();

    assert_eq!(metadata.title.as_deref(), Some("Practical metadata"));
    assert_eq!(
        metadata.description.as_deref(),
        Some("A useful page summary.")
    );
    assert_eq!(metadata.site_name.as_deref(), Some("example.com"));
    assert!(metadata.language.is_none());
    assert!(metadata.section.is_none());
    assert_eq!(metadata.authors, ["Ada Lovelace"]);
    assert_eq!(
        diagnostics.title.selected.as_ref().unwrap().source,
        legible::MetadataSource::OpenGraph
    );
    assert_eq!(
        diagnostics.description.selected.as_ref().unwrap().source,
        legible::MetadataSource::Twitter
    );
    assert!(
        diagnostics
            .title
            .alternatives
            .iter()
            .all(|candidate| candidate.value != "Default Title")
    );
    assert!(diagnostics.language.selected.is_none());
    assert!(diagnostics.language.alternatives.is_empty());
    assert!(diagnostics.section.selected.is_none());
    assert!(diagnostics.section.alternatives.is_empty());

    let ambiguous = extract(
        &format!(
            r#"<html><head><title>Unknown</title>
            <meta property="article:tag" content="Unknown">
            <meta property="article:tag" content="None">
            <meta name="author" content="Unknown">
            </head><body><main><h1>Unknown</h1><p>{CONTENT}</p></main></body></html>"#
        ),
        None,
    )
    .unwrap();
    assert_eq!(ambiguous.metadata().title.as_deref(), Some("Unknown"));
    assert_eq!(ambiguous.metadata().tags, ["Unknown", "None"]);
    assert_eq!(ambiguous.metadata().authors, ["Unknown"]);

    let fallbacks = Extractor::builder()
        .metadata_diagnostics(true)
        .build()
        .extract(
            &format!(
                r#"<html dir="sideways"><head><title>Default Title</title></head>
                <body><main><h1>Default Title</h1><div class="byline">Your Name</div>
                <p>{CONTENT}</p></main></body></html>"#
            ),
            None,
        )
        .unwrap();
    assert!(fallbacks.metadata().title.is_none());
    assert!(fallbacks.metadata().authors.is_empty());
    assert!(fallbacks.metadata().direction.is_none());
    let diagnostics = fallbacks.metadata_diagnostics().unwrap();
    assert!(diagnostics.title.selected.is_none());
    assert!(diagnostics.authors.selected.is_empty());
    assert!(diagnostics.direction.selected.is_none());
}

#[test]
fn normalizes_and_deduplicates_authors_across_metadata_sources() {
    let html = format!(
        r#"<html><head><title>Author normalization</title>
        <meta name="dc:creator" content="By&nbsp;ÉMILIE DU CHÂTELET">
        <meta name="author" content="By Émilie du Châtelet">
        <script type="application/ld+json">{{
          "@context":"https://schema.org", "@type":"Article",
          "headline":"Author normalization",
          "author":[{{"name":"Émilie du Châtelet"}},{{"name":"Ada Lovelace"}}]
        }}</script></head><body><main><h1>Author normalization</h1><p>{CONTENT}</p></main></body></html>"#
    );

    let page = Extractor::builder()
        .metadata_diagnostics(true)
        .build()
        .extract(&html, None)
        .unwrap();

    assert_eq!(
        page.metadata().authors,
        ["Émilie du Châtelet", "Ada Lovelace"]
    );
    let authors = &page.metadata_diagnostics().unwrap().authors;
    assert_eq!(authors.selected.len(), 2);
    assert_eq!(authors.selected[0].source, legible::MetadataSource::JsonLd);

    let confidence_mismatch = format!(
        r#"<html><head><title>Author source</title>
        <meta name="dc:creator" content="Ada Lovelace">
        <script type="application/ld+json">{{
          "@context":"https://schema.org", "@type":"WebPage",
          "name":"Author source", "author":{{"name":"ADA LOVELACE"}}
        }}</script></head><body><main><h1>Author source</h1><p>{CONTENT}</p></main></body></html>"#
    );
    let page = Extractor::builder()
        .metadata_diagnostics(true)
        .build()
        .extract(&confidence_mismatch, None)
        .unwrap();
    assert_eq!(page.metadata().authors, ["Ada Lovelace"]);
    assert_eq!(
        page.metadata_diagnostics().unwrap().authors.selected[0].source,
        legible::MetadataSource::DublinCore
    );

    let intentional_lowercase = format!(
        r#"<html><head><title>Intentional lowercase</title>
        <meta name="dc:creator" content="Bell Hooks">
        <script type="application/ld+json">{{
          "@context":"https://schema.org", "@type":"Article",
          "headline":"Intentional lowercase", "author":{{"name":"bell hooks"}}
        }}</script></head><body><main><h1>Intentional lowercase</h1><p>{CONTENT}</p></main></body></html>"#
    );
    let page = extract(&intentional_lowercase, None).unwrap();
    assert_eq!(page.metadata().authors, ["bell hooks"]);
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
