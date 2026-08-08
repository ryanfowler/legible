# Legible

[![Crates.io](https://img.shields.io/crates/v/legible.svg)](https://crates.io/crates/legible)
[![Documentation](https://docs.rs/legible/badge.svg)](https://docs.rs/legible)

Legible extracts the main article from an HTML document. It removes navigation, advertisements, sidebars, and other unrelated content. Legible is a Rust port of Mozilla's [Readability.js](https://github.com/mozilla/readability).

## Installation

Run this command:

```bash
cargo add legible
```

Or add the dependency to `Cargo.toml`:

```toml
[dependencies]
legible = "0.5"
```

## Extract an article

Use `parse` for most applications:

```rust
use legible::parse;

let html = r#"
    <html>
    <head><title>My Article</title></head>
    <body>
        <nav>Navigation</nav>
        <article>
            <h1>Article Title</h1>
            <p>This is the main content of the article.</p>
            <p>This second paragraph contains more article text.</p>
        </article>
        <footer>Footer</footer>
    </body>
    </html>
"#;

match parse(html, Some("https://example.com/articles/1"), None) {
    Ok(article) => {
        println!("Title: {}", article.title);
        println!("HTML: {}", article.content);
        println!("Markdown: {}", article.markdown_content);
        println!("Text: {}", article.text_content);
    }
    Err(error) => eprintln!("Error: {error}"),
}
```

The optional URL must be an absolute URL. Legible uses it as the base URL for relative links and media URLs. Relative URLs stay relative if you pass `None`.

## Check a document before extraction

`is_probably_readerable` performs a quick content check. The check is a heuristic. A `true` result does not guarantee successful extraction. A `false` result does not prove that the document has no article.

```rust
use legible::is_probably_readerable;

let text = "Article text. ".repeat(30);
let html = format!("<article><p>{text}</p></article>");

if is_probably_readerable(&html, None) {
    // The document probably contains an article.
}
```

This function parses the HTML. If you also want to extract the article, use `Document` to avoid a second HTML parse:

```rust
use legible::Document;

let text = "Article text. ".repeat(30);
let html = format!("<article><p>{text}</p></article>");
let document = Document::new(&html);

if document.is_probably_readerable(None) {
    match document.parse(Some("https://example.com/articles/1"), None) {
        Ok(article) => println!("Title: {}", article.title),
        Err(error) => eprintln!("Error: {error}"),
    }
}
```

The readability check borrows the `Document`. Article extraction consumes it because extraction changes the internal document tree.

## Article fields

`parse` and `Document::parse` return an `Article` with these fields:

| Field              | Type             | Description                                            |
| ------------------ | ---------------- | ------------------------------------------------------ |
| `title`            | `String`         | Article title                                          |
| `content`          | `String`         | Extracted HTML; not sanitized                          |
| `markdown_content` | `String`         | CommonMark without raw HTML or unsupported URI schemes |
| `text_content`     | `String`         | Normalized plain text                                  |
| `byline`           | `Option<String>` | Author byline                                          |
| `excerpt`          | `Option<String>` | Short article excerpt                                  |
| `site_name`        | `Option<String>` | Site name                                              |
| `published_time`   | `Option<String>` | Publication time from the source metadata              |
| `dir`              | `Option<String>` | Text direction, such as `ltr` or `rtl`                 |
| `lang`             | `Option<String>` | Document language, such as `en` or `fr`                |
| `length`           | `usize`          | Number of characters in `text_content`                 |

## Configure extraction

Use the `Options` builder and pass the result to `parse`:

```rust
use legible::{Options, parse};

let options = Options::new()
    .char_threshold(250)
    .keep_classes(true)
    .disable_json_ld(true);

let result = parse(
    "<html><body><article><p>Article text</p></article></body></html>",
    Some("https://example.com/articles/1"),
    Some(options),
);
```

Extraction options have these defaults:

| Option                  | Default    | Effect                                                                                          |
| ----------------------- | ---------- | ----------------------------------------------------------------------------------------------- |
| `max_elems_to_parse`    | `0`        | Sets the maximum number of HTML elements to analyze. `0` sets no limit.                         |
| `nb_top_candidates`     | `5`        | Sets the number of high-score content candidates to compare.                                    |
| `char_threshold`        | `500`      | Sets the target minimum article length. Legible retries with less filtering below this value.   |
| `keep_classes`          | `false`    | Keeps all CSS classes when set to `true`.                                                       |
| `classes_to_preserve`   | `["page"]` | Lists CSS classes to keep when `keep_classes` is `false`. The builder method extends this list. |
| `disable_json_ld`       | `false`    | Disables JSON-LD metadata extraction when set to `true`.                                        |
| `allowed_video_regex`   | `None`     | Uses a built-in list. A custom regular expression replaces that list.                           |
| `link_density_modifier` | `0.0`      | Changes link-density limits. A positive value keeps more link-heavy content.                    |
| `debug`                 | `false`    | Writes extraction decisions to standard error when set to `true`.                               |

`char_threshold` is a retry threshold, not a strict minimum. After all retries, Legible can return shorter nonempty content.

You can also configure the quick readability check:

```rust
use legible::{ReaderableOptions, is_probably_readerable};

let options = ReaderableOptions::new()
    .min_score(30.0)
    .min_content_length(100);

let text = "Article text. ".repeat(30);
let html = format!("<article><p>{text}</p></article>");
let likely_article = is_probably_readerable(&html, Some(options));
```

`min_score` defaults to `20.0`. `min_content_length` defaults to `140` characters.

## Security

**Do not render `Article::content` without sanitizing it.**

Legible cleans article content, but it is not an HTML security sanitizer. The HTML can contain unsafe attributes, URLs, or other source markup. Apply a sanitizer that matches your security policy before you render the HTML. For example, you can use [ammonia](https://docs.rs/ammonia):

```rust
let article = legible::parse(html, Some(url), None)?;
let safe_html = ammonia::clean(&article.content);
```

`markdown_content` does not contain raw HTML. It removes links and images that have unsupported URI schemes. Links can use HTTP, HTTPS, email, telephone, fragment, and relative destinations. Images can use HTTP, HTTPS, and relative destinations. If you convert the Markdown to HTML, sanitize that HTML according to your application's security policy.

## How Legible works

Legible uses the Readability.js extraction process:

1. It parses the HTML and prepares the document tree.
2. It reads metadata from JSON-LD, OpenGraph properties, and meta elements.
3. It scores content from its element type, text density, links, classes, and identifiers.
4. It selects the content container with the highest score.
5. It removes low-score elements, empty containers, and unrelated markup.

The test suite includes Mozilla's official [Readability.js test pages](https://github.com/mozilla/readability/tree/main/test/test-pages).

## License

Apache-2.0
