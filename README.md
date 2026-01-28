# Legible

A Rust port of Mozilla's [Readability.js](https://github.com/mozilla/readability) for extracting readable content from web pages.

Legible analyzes HTML documents and extracts the main article content, stripping away navigation, ads, sidebars, and other non-content elements to produce clean, readable output.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
legible = "0.1"
```

## Usage

### Basic Extraction

```rust
use legible::Readability;

let html = r#"
    <html>
    <head><title>My Article</title></head>
    <body>
        <nav>Navigation</nav>
        <article>
            <h1>Article Title</h1>
            <p>This is the main content of the article...</p>
        </article>
        <footer>Footer</footer>
    </body>
    </html>
"#;

let readability = Readability::new(html, Some("https://example.com"), None);
match readability.parse() {
    Ok(article) => {
        println!("Title: {}", article.title);
        println!("Content: {}", article.content);
        println!("Text: {}", article.text_content);
    }
    Err(e) => eprintln!("Error: {}", e),
}
```

### Quick Readability Check

Before running the full extraction, you can check if a document is likely to contain readable content:

```rust
use legible::is_probably_readerable;

if is_probably_readerable(html, None) {
    // Document appears to have extractable content
}
```

### Extracted Article Fields

The `Article` struct contains:

| Field            | Type             | Description                       |
| ---------------- | ---------------- | --------------------------------- |
| `title`          | `String`         | The article title                 |
| `content`        | `String`         | The article content as HTML       |
| `text_content`   | `String`         | The article content as plain text |
| `byline`         | `Option<String>` | The author byline                 |
| `excerpt`        | `Option<String>` | A short excerpt from the article  |
| `site_name`      | `Option<String>` | The site name                     |
| `published_time` | `Option<String>` | The published time                |
| `dir`            | `Option<String>` | Text direction (ltr or rtl)       |
| `lang`           | `Option<String>` | Document language                 |
| `length`         | `usize`          | Length of the text content        |

## Configuration

Use the `Options` builder to customize parsing behavior:

```rust
use legible::{Readability, Options};

let options = Options::new()
    .char_threshold(250)        // Minimum article length (default: 500)
    .keep_classes(true)         // Preserve CSS classes in output
    .disable_json_ld(true);     // Skip JSON-LD metadata extraction

let readability = Readability::new(html, Some(url), Some(options));
```

### Available Options

| Option                  | Default    | Description                               |
| ----------------------- | ---------- | ----------------------------------------- |
| `max_elems_to_parse`    | `0`        | Maximum elements to parse (0 = unlimited) |
| `nb_top_candidates`     | `5`        | Number of top candidates to consider      |
| `char_threshold`        | `500`      | Minimum article character length          |
| `keep_classes`          | `false`    | Preserve CSS classes in output            |
| `classes_to_preserve`   | `["page"]` | Specific classes to keep                  |
| `disable_json_ld`       | `false`    | Skip JSON-LD metadata extraction          |
| `allowed_video_regex`   | -          | Custom regex for allowed video embeds     |
| `link_density_modifier` | `0.0`      | Adjust link density threshold             |
| `debug`                 | `false`    | Enable debug logging                      |

## Security

The extracted HTML content is **unsanitized** and may contain malicious scripts or other dangerous content from the source document. Before rendering this HTML in a browser or other context where scripts could execute, you should sanitize it using a library like [ammonia](https://docs.rs/ammonia):

```rust
use legible::Readability;

let readability = Readability::new(html, Some(url), None);
let article = readability.parse()?;

// Sanitize before rendering
let safe_html = ammonia::clean(&article.content);
```

## How It Works

Legible implements the same algorithm as Readability.js:

1. **Document Preparation** - Removes scripts, normalizes markup, fixes lazy-loaded images
2. **Metadata Extraction** - Extracts title, byline, and other metadata from JSON-LD, OpenGraph tags, and meta elements
3. **Content Scoring** - Scores DOM nodes based on tag type, text density, and class/id patterns
4. **Candidate Selection** - Identifies the highest-scoring content container
5. **Content Cleaning** - Removes low-scoring elements, empty containers, and non-content markup

The library is tested against Mozilla's official [Readability.js test suite](https://github.com/mozilla/readability/tree/main/test/test-pages).

## License

Apache-2.0
