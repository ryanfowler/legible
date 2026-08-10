#![allow(dead_code)]

use std::borrow::Cow;

use legible::extract;

/// Keep one fuzz case small enough to avoid turning an input-size mutation into an
/// allocation test. The fuzzer can still find large structural cases by nesting
/// markup instead of repeating large text.
pub const MAX_INPUT_SIZE: usize = 256 * 1024;

pub fn input(data: &[u8]) -> Option<Cow<'_, str>> {
    (data.len() <= MAX_INPUT_SIZE).then(|| String::from_utf8_lossy(data))
}

pub fn article_document(body: &str) -> String {
    format!(
        "<html><head><title>Fuzz article</title></head><body>\
         <article><h1>Fuzz article</h1><p>Article text starts here. {body} \
         This paragraph contains enough text for extraction. </p></article>\
         </body></html>"
    )
}

pub fn parse_article(html: &str) -> Option<legible::ExtractedPage> {
    extract(html, Some("https://example.com/articles/index.html")).ok()
}

/// Exercise the public serialization boundary. Extracted HTML is a fragment, so a
/// successful parse is the reparsing invariant for the final serializer.
pub fn reparse_serialized(content: &str) {
    let _ = extract(content, None);
}

pub fn escape_attribute(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '"' => escaped.push_str("&quot;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            _ => escaped.push(character),
        }
    }
    escaped
}
