# Legible

[![Crates.io](https://img.shields.io/crates/v/legible.svg)](https://crates.io/crates/legible)
[![Documentation](https://docs.rs/legible/badge.svg)](https://docs.rs/legible)

Legible extracts relevant content and metadata from HTML. It stores a cleaned DOM
subtree. It renders Markdown, HTML, or normalized text only when you request that
format.

Legible uses general semantic candidates, source-relative quality checks, and
conservative fallbacks. Mozilla Readability is an important algorithmic ancestor,
but article-style prose is not required.

Legible has no browser engine. It does not execute JavaScript or make network
requests. Extraction is deterministic for the same input and configuration.

Reject unsuccessful HTTP status codes before you pass a response body to Legible.
Legible does not receive the transport status. It rejects access barriers only when
the HTML contains enough structural and textual evidence.

## Extract content

```rust
use legible::extract;

let html = r#"
<html lang="en">
  <head><title>Building a cache</title></head>
  <body>
    <nav>Navigation</nav>
    <main>
      <p>This page explains how to build a cache.</p>
    </main>
  </body>
</html>
"#;

let page = extract(html, Some("https://example.com/cache"))?;

println!("{}", page.markdown());
println!("{}", page.text());
println!("{}", page.html());

if let Some(title) = &page.metadata().title {
    println!("{title}");
}

# Ok::<(), legible::Error>(())
```

The optional URL must be absolute. Legible uses it as the base URL for relative
links and media URLs. Relative URLs stay relative when you pass `None`.

## Configure extraction

Use one `Extractor` for pages that share a configuration.

```rust
use legible::Extractor;

let extractor = Extractor::builder()
    .max_elements(100_000)
    .structured_data(true)
    .build();

let page = extractor.extract("<main><p>Page content.</p></main>", None)?;
# Ok::<(), legible::Error>(())
```

`max_elements(0)` sets no limit. Structured-data metadata extraction is enabled by
default.

Enable structured decision diagnostics only when you need them:

```rust
# use legible::Extractor;
let extractor = Extractor::builder().diagnostics(true).build();
let page = extractor.extract("<main><p>Page content.</p></main>", None)?;
if let Some(diagnostics) = page.diagnostics() {
    println!("Selected {:?}", diagnostics.selected_strategy);
    println!("Specialized extractor: {:?}", diagnostics.specialized_extractor);
    for attempt in &diagnostics.attempts {
        println!("Cleanup: {:?}", attempt.cleanup_actions);
        println!("Normalization: {:?}", attempt.normalization);
    }
}
# Ok::<(), legible::Error>(())
```

Legible does not retain attempt diagnostics by default. When enabled, diagnostics record each strategy, the selected root, quality metrics, major cleanup actions, semantic normalization counts, and the specialized extractor identity.

Use a content hint when you know a likely content container. The hint adds strong
evidence, but Legible can select a better container. Use `content_root` when you
must limit extraction to one matching subtree.

```rust
# use legible::{ContentHint, Extractor};
let extractor = Extractor::builder()
    .content_hint(ContentHint::Class("article-body".into()))
    .build();
# let _ = extractor;
```

## Render Markdown

`page.markdown()` includes links and images. Use the builder to change these settings.

```rust
# let page = legible::extract("<main><p>Text</p></main>", None)?;
let markdown = page
    .markdown_builder()
    .links(false)
    .images(false)
    .render();
# Ok::<(), legible::Error>(())
```

## Supported pages

Legible handles articles, documentation, API references, indexes, listings, code,
tables, figures, and short pages. It falls back to a conservatively cleaned body
when a page has useful content but no clear primary container.

## Metadata

`page.metadata()` returns a `Metadata` reference. It can contain:

- title and description
- multiple authors
- site name and canonical URL
- image and favicon URLs
- publication and modification times
- language and text direction
- section and tags

Missing values stay empty or `None`.

Enable `metadata_diagnostics(true)` to retain the selected source, confidence, and
alternatives. Enable `retain_structured_data(true)` to retain parsed JSON-LD items.
Both options are disabled by default.

## Security

`ExtractedPage::safe_html()` removes active elements, event handlers, inline styles,
unsafe form actions, and unsupported URI schemes. Use it for normal downstream
display. `ExtractedPage::html()` remains unsanitized for compatibility. Do not insert
raw HTML into an untrusted page.

Markdown output contains no raw HTML. It removes links and images that use unsupported
URI schemes. Sanitize any HTML that you later create from the Markdown.

Legible does not fetch URLs.

## Regression fixtures

`tests/general/` contains exact Markdown fixtures. `tests/web/` contains capability
fixtures with semantic assertions in `expected.json`. Add focused positive and
negative cases for each extraction heuristic.

Install and run the optional quality comparison tool with:

```bash
npm --prefix scripts/compare-extractors ci
cargo fetch
node scripts/compare-extractors/index.mjs --all
```

The tool compares Legible with pinned third-party extractors against independent
quality fixtures. See `benchmarks/quality/README.md` for the fixture format and
`scripts/compare-extractors/README.md` for runner options.

Run the compatibility performance suite with:

```bash
cargo bench --bench extraction
```

See `benches/README.md` for workloads, baseline commands, and performance guardrails.

## License

Apache-2.0. See [LICENSE](LICENSE).
