# Legible

[![Crates.io](https://img.shields.io/crates/v/legible.svg)](https://crates.io/crates/legible)
[![Documentation](https://docs.rs/legible/badge.svg)](https://docs.rs/legible)

Legible extracts relevant content and metadata from HTML. It compiles selected HTML
into a private semantic representation. It renders Markdown, canonical HTML, or
normalized text from that representation only when you request the format.

Legible uses general semantic candidates, source-relative quality checks, and
conservative fallbacks. Mozilla Readability is an important algorithmic ancestor,
but article-style prose is not required.

Legible has no browser engine. It does not execute JavaScript or make network
requests. Extraction is deterministic for the same input and configuration.

Reject unsuccessful HTTP status codes before you pass a response body to Legible.
Legible does not receive the transport status. It rejects access barriers only when
the HTML contains enough structural and textual evidence.

## API at a glance

The crate has four main public entry points:

- `extract(html, url)` performs one extraction with the default configuration.
- `Extractor` stores configuration that you can reuse for many documents.
- `ExtractedPage` provides Markdown, HTML, text, metadata, diagnostics, and metrics.
- `ParseBudget` limits parser and JSON-LD resource use.

The extracted representation is private. You can use the output methods without
depending on the internal document model.

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
default. For resource-constrained callers, use `ParseBudget` or the builder's
budget methods to limit input bytes, DOM nodes, attributes, text, nesting depth,
and JSON-LD work. A value of `0` means no caller-configured limit. JSON-LD depth
still has an internal safety cap.

You can set all limits with `ParseBudget`:

```rust
use legible::{Extractor, ParseBudget};

let budget = ParseBudget {
    max_input_bytes: 10 * 1024 * 1024,
    max_nodes: 200_000,
    max_elements: 100_000,
    max_total_attributes: 500_000,
    max_attributes_per_element: 200,
    max_text_bytes: 8 * 1024 * 1024,
    max_depth: 512,
    max_json_ld_bytes: 2 * 1024 * 1024,
    max_json_ld_items: 10_000,
    max_json_ld_depth: 128,
};

let extractor = Extractor::builder()
    .parse_budget(budget)
    .build();
```

The limits apply to the input document and its JSON-LD. Legible does not fetch
resources. A resource limit returns `Error::ResourceLimit`.

## Select content

Legible selects the most relevant content region by default. Use a hint when you
know a likely container:

```rust
use legible::{ContentHint, Extractor};

let extractor = Extractor::builder()
    .content_hint(ContentHint::Class("article-body".into()))
    .build();
```

The hint adds evidence. Quality checks still apply. `ContentHint::Id` matches one
exact ID. `ContentHint::Class` matches one class token. `ContentHint::Tag` matches
`article`, `main`, `section`, or `div` elements.

Use `content_root` when you must extract one matching subtree. It selects the
first matching element and returns `Error::ContentRootNotFound` when no element
matches. This option keeps the requested boundary and does not perform normal
automatic root selection outside that subtree.

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
        println!("Semantic coverage: {:?}", attempt.semantic_coverage);
    }
}
# Ok::<(), legible::Error>(())
```

Legible does not retain attempt diagnostics by default. When enabled, diagnostics record each strategy, the selected root, quality metrics, candidate-to-result semantic coverage, major cleanup actions, semantic normalization counts, representation sizes, and the specialized extractor identity. Semantic coverage is diagnostic data. It does not affect attempt acceptance.

## Outputs and metrics

Legible's semantic representation is an internal implementation detail. Public
output contracts are Markdown, canonical semantic HTML, normalized text, metadata,
and scalar metrics. Content methods return Markdown, canonical semantic HTML, or
normalized text. Metadata and scalar metrics are also available on `ExtractedPage`:

```rust
# let page = legible::extract("<main><p>Page content.</p></main>", None)?;
println!("{} words", page.word_count());
println!("{} characters", page.text_length());
println!("{} images", page.image_count());
# Ok::<(), legible::Error>(())
```

The representation can change without a public API change.

`page.markdown()` includes links and images. `page.html()` returns canonical
semantic HTML. It contains no source scripts, event handlers, arbitrary source
attributes, or unsupported URI schemes. `page.safe_html()` is a compatibility
alias for `page.html()`.

`page.text()` returns normalized plain text. Repeated output calls are
deterministic. Rendering is lazy, so Legible does not create all output formats
unless you request them.

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

`structured_data(true)` controls whether JSON-LD can affect metadata and content
selection. It is enabled by default. `retain_structured_data(true)` controls only
whether parsed items remain available through `page.structured_data()`. When
retention is disabled, that method returns `None`. When retention is enabled, it
returns `Some`, including when the slice is empty.

## Errors

Extraction returns `Result<ExtractedPage, Error>`. The main errors are:

- `InvalidUrl` when the optional base URL is not absolute or cannot be parsed.
- `NoBody` when the parsed document has no body.
- `NoContent` when Legible cannot find useful content.
- `ContentRootNotFound` when an exact configured root is absent.
- `TooManyElements` or `ResourceLimit` when a configured limit is exceeded.
- `Parse` when the HTML cannot be converted into the internal document.

Reject unsuccessful HTTP responses before extraction. Legible receives only the
HTML body and does not know the transport status.

## Optional features

- `tracing` emits debug events for extraction decisions. Add a `tracing` subscriber
  in your application to collect them.
- `bench-instrumentation` exposes phase timings and allocation counters for
  benchmark work. It adds measurement state and is not needed for normal use.

## Security

`ExtractedPage::html()` returns canonical semantic HTML. The private semantic
representation cannot contain active source elements, event handlers, arbitrary
source attributes, or unsupported URI schemes. `ExtractedPage::safe_html()` is an
alias for the same output.

Markdown output contains no raw HTML. The semantic compiler rejects links and media
that use unsupported URI schemes. Sanitize HTML that you create from other sources.

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
