#![no_main]

use libfuzzer_sys::fuzz_target;

mod support;

use support::{article_document, input, parse_article, reparse_serialized};

fuzz_target!(|data: &[u8]| {
    let Some(body) = input(data) else { return };
    let html = article_document(body.as_ref());

    // Extraction performs the DOM detach, append, replace, and cleanup operations.
    let Some(page) = parse_article(&html) else {
        return;
    };
    reparse_serialized(&page.html());
});
