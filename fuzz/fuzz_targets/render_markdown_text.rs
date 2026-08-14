#![no_main]

use libfuzzer_sys::fuzz_target;

mod support;

use support::{article_document, input, parse_article, reparse_serialized};

fuzz_target!(|data: &[u8]| {
    let Some(body) = input(data) else { return };
    let html = article_document(body.as_ref());

    // Render each output from the retained semantic document.
    let Some(page) = parse_article(&html) else {
        return;
    };
    let _markdown = page.markdown();
    let _text = page.text();
    reparse_serialized(&page.html());
});
