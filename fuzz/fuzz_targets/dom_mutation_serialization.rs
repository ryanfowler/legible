#![no_main]

use libfuzzer_sys::fuzz_target;

mod support;

use support::{article_document, input, parse_article, reparse_serialized};

fuzz_target!(|data: &[u8]| {
    let Some(body) = input(data) else { return };
    let html = article_document(body.as_ref());

    // Article extraction performs the DOM detach, append, replace, and cleanup
    // operations. Its content is produced by the direct iterative serializer.
    let Some(article) = parse_article(&html) else {
        return;
    };
    reparse_serialized(&article.content);
});
