#![no_main]

use libfuzzer_sys::fuzz_target;

mod support;

use support::{input, parse_article, reparse_serialized};

fuzz_target!(|data: &[u8]| {
    let Some(json) = input(data) else { return };
    let html = format!(
        "<html><head><title>Fuzz article</title>\
         <script type=\"application/ld+json\">{}</script></head>\
         <body><article><p>Article text for metadata extraction. \
         This paragraph contains enough text for the parser.</p></article></body></html>",
        json.as_ref()
    );

    // The script is intentionally not escaped. This also fuzzes malformed JSON,
    // premature script terminators, and HTML/JSON boundary handling.
    let Some(page) = parse_article(&html) else {
        return;
    };
    let metadata = page.metadata();
    let _ = metadata.title.as_deref();
    let _ = metadata.authors.as_slice();
    let _ = metadata.description.as_deref();
    let _ = metadata.site_name.as_deref();
    let _ = metadata.published_time.as_deref();
    reparse_serialized(&page.html());
});
