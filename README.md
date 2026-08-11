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
}
# Ok::<(), legible::Error>(())
```

Legible does not retain attempt diagnostics by default.

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

## Security

`ExtractedPage::html()` is not sanitized. Do not insert its output into an untrusted
page without a sanitizer that matches your security policy.

Markdown output contains no raw HTML. It removes links and images that use unsupported
URI schemes. Sanitize any HTML that you later create from the Markdown.

Legible does not fetch URLs.

## Regression fixtures

`tests/general/` contains exact Markdown fixtures. `tests/web/` contains capability
fixtures with semantic assertions in `expected.json`. Add focused positive and
negative cases for each extraction heuristic.

Run the optional maintainer comparison tool with:

```bash
node scripts/compare-defuddle/index.mjs
```

See `scripts/compare-defuddle/README.md` for optional Defuddle comparison.

## License

Apache-2.0. See [LICENSE](LICENSE).
