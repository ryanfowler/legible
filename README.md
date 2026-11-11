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
- `ExtractedPage` provides Markdown, HTML, text, metadata, and diagnostics. Its
  `content()` value also provides streaming writers and content metrics.
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
and content metrics. `ExtractedPage` provides `markdown()`, `markdown_builder()`,
`html()`, and `text()`. `page.content()` returns the `ExtractedContent` value.
It provides the same output methods, the streaming writers, and `metrics()`:

```rust
# let page = legible::extract("<main><p>Page content.</p></main>", None)?;
let metrics = page.content().metrics();
println!("{} words", metrics.word_count);
println!("{} characters", metrics.text_length);
println!("{} images", metrics.image_count);
# Ok::<(), legible::Error>(())
```

`metrics()` returns a `ContentMetrics` value. The first call measures the
normalized text once. Later calls and text rendering reuse that measurement.

Use `page.into_parts()` when each result needs a separate owner. The returned
`ExtractedPageParts` contains the metadata, diagnostics, structured data, and
`ExtractedContent`.

The representation can change without a public API change.

`page.markdown()` includes links and images. `page.html()` returns canonical
semantic HTML. It contains no source scripts, event handlers, arbitrary source
attributes, or unsupported URI schemes.

`page.text()` returns normalized plain text. Repeated output calls are
deterministic. Rendering is lazy, so Legible does not create all output formats
unless you request them.

Use `page.content().write_markdown`, `write_html`, or `write_text` when an API
accepts a `std::fmt::Write` value. These methods return `fmt::Result` and write directly
to the provided value. They do not create a complete intermediate output
`String`.

```rust
# let page = legible::extract("<main><p>Text</p></main>", None).unwrap();
use std::fmt::Write;

let mut output = String::new();
page.content().write_markdown(&mut output).unwrap();
```

Use `write_markdown_io`, `write_html_io`, or `write_text_io` when an API accepts
a `std::io::Write` value. These methods write UTF-8 bytes directly and return
`std::io::Result<()>`. The Markdown builder methods named `write` and `write_io`
apply the builder's output configuration.

```rust,no_run
# let page = legible::extract("<main><p>Text</p></main>", None).unwrap();
let mut output = std::io::BufWriter::new(std::fs::File::create("output.md").unwrap());
page.content().write_markdown_io(&mut output).unwrap();
```

## Render Markdown

`page.markdown()` includes links and images. It does not wrap lines. Use the
builder to change these settings.

```rust
# let page = legible::extract("<main><p>Text</p></main>", None)?;
let markdown = page
    .markdown_builder()
    .links(false)
    .images(false)
    .max_line_width(80)
    .render();
# Ok::<(), legible::Error>(())
```

The line width is a preferred maximum. The renderer wraps prose at whitespace.
It keeps URLs, code spans, code blocks, headings, math blocks, and tables intact.
These items can exceed the selected width.

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
- `ResourceLimit` when a configured limit is exceeded. Its `resource` field is a
  `ResourceLimitKind` value, and its `limit` field contains the configured
  maximum.
- `Parse` when the HTML cannot be converted into the internal document.

`Error` is non-exhaustive. Include a wildcard arm when you match it. The
`InvalidUrl` variant holds an `InvalidUrlError` value. Its display text describes
the problem, and it does not expose the URL parser type.

For example, callers can handle resource limits without parsing display strings:

```rust
use legible::{Error, ResourceLimitKind};

fn report_error(error: Error) {
    match error {
        Error::ResourceLimit {
            resource: ResourceLimitKind::Elements,
            limit,
        } => eprintln!("the document has more than {limit} elements"),
        Error::ResourceLimit {
            resource: ResourceLimitKind::JsonLdBytes,
            limit,
        } => eprintln!("JSON-LD exceeds the {limit}-byte limit"),
        Error::ResourceLimit { resource, limit } => {
            eprintln!("{resource:?} exceeds the {limit} limit");
        }
        _ => {}
    }
}
```

Reject unsuccessful HTTP responses before extraction. Legible receives only the
HTML body and does not know the transport status.

## Command-line client

The `cli/` package provides a `legible` executable. It fetches one HTTP or HTTPS
URL, converts the response to UTF-8, and writes Markdown to standard output. The
output starts with YAML frontmatter containing the available page metadata. The
Markdown renderer uses the smaller of the terminal width and 100 columns.

```bash
cargo run --manifest-path cli/Cargo.toml -- https://example.com/article
```

## Optional features

- `tracing` emits debug events for extraction decisions. Add a `tracing` subscriber
  in your application to collect them.

## Security

`ExtractedPage::html()` returns canonical semantic HTML. The private semantic
representation cannot contain active source elements, event handlers, arbitrary
source attributes, or unsupported URI schemes.

Markdown output contains no raw HTML. The semantic compiler rejects links and media
that use unsupported URI schemes. Sanitize HTML that you create from other sources.

Legible does not fetch URLs.

## Tests and evaluations

`tests/fixtures/snapshots/` contains exact Markdown and error fixtures.
`tests/fixtures/capabilities/` contains focused semantic assertions. Run both
repository fixture types with:

```bash
cargo test --test fixture_tests
```

The Mozilla corpus has a separate tolerant compatibility runner. See
`tests/README.md` for all test suites and guidance for new cases.

Install and run the optional quality comparison tool with:

```bash
npm --prefix tools/extractor-eval ci
cargo fetch
node tools/extractor-eval/index.mjs --all
```

The tool compares Legible with pinned third-party extractors against independent
quality fixtures. See `evals/quality/README.md` for the fixture format and
`tools/extractor-eval/README.md` for runner options.

Run the compatibility performance suite with:

```bash
cargo bench --bench pipeline
```

See `benches/README.md` for workloads, baseline commands, and performance guardrails.

## License

Apache-2.0. See [LICENSE](LICENSE).
