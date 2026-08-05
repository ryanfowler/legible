# Legible

[![Crates.io](https://img.shields.io/crates/v/legible.svg)](https://crates.io/crates/legible)
[![Documentation](https://docs.rs/legible/badge.svg)](https://docs.rs/legible)

Legible extracts the main article from an HTML document. It is a Rust port of Mozilla's Readability.js.

## Installation

```toml
[dependencies]
legible = "0.5"
```

## Extract an article

Use the function for HTML output:

```rust
let article = legible::extract_html(html)?;
println!("{}", article.metadata().title().unwrap_or("Untitled"));
println!("{}", article.content());
# Ok::<(), legible::Error>(())
```

Use `Extractor` for Markdown or normalized text:

```rust
use legible::Extractor;

let extractor = Extractor::default();
let markdown = extractor.extract_markdown(html)?;
let text = extractor.extract_text(html)?;
# Ok::<(), legible::Error>(())
```

Each extraction method creates only its requested format. Run extraction again if you need another public format. The deprecated `parse` adapter can still create all three formats in one extraction.

Use a typed base URL to resolve relative links and media URLs:

```rust
use url::Url;
let url = Url::parse("https://example.com/articles/1")?;
let article = legible::extract_html_with_url(html, &url)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

`text_char_count()` returns the normalized source text character count. Text options can add line breaks without changing this count.

## Reuse extraction configuration

```rust
use legible::{ClassPolicy, Extractor};

let extractor = Extractor::builder()
    .max_input_bytes(2 * 1024 * 1024)
    .max_elements(50_000)
    .retry_length_threshold(250)
    .class_policy(ClassPolicy::StripSourceClasses)
    .build()?;

let article = extractor.extract_html(html)?;
# Ok::<(), legible::Error>(())
```

The retry length threshold causes less-filtered retries. It is not a strict output minimum.

## Check readability without parsing twice

The quick check is a heuristic. It can return false positives or false negatives.

```rust
use legible::{Document, Extractor};

let document = Document::parse(html)?;
if document.is_probably_readable() {
    let article = Extractor::default().extract_document_html(document)?;
}
# Ok::<(), legible::Error>(())
```

For a one-step check, use `is_probably_readable(html, options)`.

## Metadata

Use `article.metadata()`. Metadata includes the title, byline, authors, excerpt, site name, publisher, canonical URL, lead image, publication and modification times, section, tags, language, and text direction.

## Security

**`HtmlArticle::content()` is not sanitized.**

Legible cleans article content, but it is not an HTML security sanitizer. Apply a sanitizer that matches your security policy before you render the HTML.

```rust
let safe_html = ammonia::clean(article.content());
```

Markdown does not contain raw HTML. Legible removes destinations that use unsupported URI schemes. Sanitize HTML that you create from Markdown.

## Legacy API

Version 0.5 keeps the deprecated JavaScript-shaped adapter:

```rust
#[allow(deprecated)]
let article = legible::parse(html, None, None)?;
println!("{}", article.content);
# Ok::<(), legible::Error>(())
```

The old result type and options also exist under `legible::legacy`.

## How Legible works

1. Legible parses HTML into a mutable extraction DOM.
2. It collects metadata and scores article candidates.
3. It cleans the selected article subtree.
4. It renders the requested format directly from the cleaned DOM.
5. It drops the DOM before it returns the result.

The test suite includes Mozilla's official Readability.js fixtures.

## License

Apache-2.0
