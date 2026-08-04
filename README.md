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

```rust
let article = legible::extract(html)?;
println!("{}", article.title().unwrap_or("Untitled"));

// Legible creates only the format that you request.
let markdown = article.to_markdown();
# Ok::<(), legible::Error>(())
```

Use a typed base URL to resolve relative links and media URLs:

```rust
use url::Url;
let url = Url::parse("https://example.com/articles/1")?;
let article = legible::extract_with_url(html, &url)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

`Article` owns a compact immutable article tree and metadata. Use `to_html()`, `to_markdown()`, and `to_text()` to create output on demand. Use `text_char_count()` to get the normalized text character count without rendering text.

## Reuse extraction configuration

```rust
use legible::{ClassPolicy, Extractor};

let extractor = Extractor::builder()
    .max_input_bytes(2 * 1024 * 1024)
    .max_elements(50_000)
    .retry_length_threshold(250)
    .class_policy(ClassPolicy::StripSourceClasses)
    .build()?;

let article = extractor.extract(html)?;
# Ok::<(), legible::Error>(())
```

The retry length threshold causes less-filtered retries. It is not a strict output minimum.

## Check readability without parsing twice

The quick check is a heuristic. It can return false positives or false negatives.

```rust
use legible::{Document, Extractor};

let document = Document::parse(html)?;
if document.is_probably_readable() {
    let article = Extractor::default().extract_document(document)?;
}
# Ok::<(), legible::Error>(())
```

For a one-step check, use `is_probably_readable(html, options)`.

## Metadata

Use `article.metadata()` or the common convenience methods. Metadata includes the title, byline, authors, excerpt, site name, publisher, canonical URL, lead image, publication and modification times, section, tags, language, and text direction.

## Security

**The value from `Article::to_html()` is not sanitized.**

Legible cleans article content, but it is not an HTML security sanitizer. Apply a sanitizer that matches your security policy before you render the HTML.

```rust
let safe_html = ammonia::clean(&article.to_html());
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
4. It copies only reachable output nodes into an immutable `ArticleTree`.
5. It drops the mutable source DOM.
6. It renders output only when requested.

The test suite includes Mozilla's official Readability.js fixtures.

## License

Apache-2.0
